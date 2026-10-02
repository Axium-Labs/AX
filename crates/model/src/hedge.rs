//! Adaptive hedging for the Fast inference mode.
//!
//! [`HedgingProvider`] wraps one primary provider plus ranked alternates
//! serving the same model. A streaming request goes to the primary first;
//! if no valid text/reasoning/tool-call delta arrives within the adaptive
//! threshold (learned from the primary's historical TTFT, overridable in
//! config), one secondary request is started. The first candidate to produce
//! a valid delta wins, every loser is canceled immediately, and only the
//! winner's response — including its tool calls — reaches the agent loop.
//!
//! Fast mode never alters the request itself: same model, same reasoning
//! effort, same output limits. It only changes *when* answers arrive.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{ContentPart, ModelError, ModelProvider, ModelRequest, ModelResponse, stats};

/// Knobs for [`HedgingProvider`]. Everything has a sensible default; ordinary
/// users never configure any of this.
#[derive(Clone, Copy, Debug)]
pub struct HedgeConfig {
    /// Fixed hedge delay. `None` (the default) derives the threshold from the
    /// primary provider's historical TTFT — see [`stats::hedge_threshold`].
    pub threshold_override: Option<Duration>,
    /// Maximum concurrent inference requests. 2 (the default) means primary
    /// plus one secondary; 1 disables hedging entirely.
    pub max_parallel: usize,
}

impl Default for HedgeConfig {
    fn default() -> Self {
        Self {
            threshold_override: None,
            max_parallel: 2,
        }
    }
}

/// A provider that races identical requests against each other to cut tail
/// latency, as described in the module docs.
pub struct HedgingProvider {
    primary: Arc<dyn ModelProvider>,
    /// Other providers serving the same model, best first. May be empty, in
    /// which case the hedge fires a second request at the primary itself.
    alternates: Vec<Arc<dyn ModelProvider>>,
    config: HedgeConfig,
}

enum RaceEvent {
    Delta {
        index: usize,
        text: String,
        thinking: bool,
    },
    Done {
        index: usize,
        result: Result<ModelResponse, ModelError>,
    },
}

struct Candidate {
    index: usize,
    started: Instant,
    handle: JoinHandle<()>,
}

impl HedgingProvider {
    #[must_use]
    pub fn new(
        primary: Arc<dyn ModelProvider>,
        mut alternates: Vec<Arc<dyn ModelProvider>>,
        config: HedgeConfig,
    ) -> Self {
        // Best historical composite (TTFT, tokens/s, error rate) first.
        alternates.sort_by(|left, right| {
            stats::provider_perf(left.name(), left.model_id())
                .hedge_score()
                .total_cmp(&stats::provider_perf(right.name(), right.model_id()).hedge_score())
        });
        Self {
            primary,
            alternates,
            config,
        }
    }

    fn threshold(&self) -> Duration {
        self.config
            .threshold_override
            .unwrap_or_else(|| stats::hedge_threshold(self.primary.name(), self.primary.model_id()))
    }

    fn secondary(&self) -> Arc<dyn ModelProvider> {
        self.alternates
            .first()
            .cloned()
            .unwrap_or_else(|| self.primary.clone())
    }

    fn spawn_candidate(
        candidates: &mut Vec<Candidate>,
        provider: Arc<dyn ModelProvider>,
        request: &ModelRequest,
        tx: &mpsc::UnboundedSender<RaceEvent>,
    ) {
        let index = candidates.len();
        let done_sender = tx.clone();
        let request = request.clone();
        let handle = tokio::spawn(async move {
            let delta_sender = done_sender.clone();
            let thinking_sender = done_sender.clone();
            let mut on_delta = move |text: String| {
                let _ = delta_sender.send(RaceEvent::Delta {
                    index,
                    text,
                    thinking: false,
                });
            };
            let mut on_thinking = move |text: String| {
                let _ = thinking_sender.send(RaceEvent::Delta {
                    index,
                    text,
                    thinking: true,
                });
            };
            let result = provider
                .complete_stream(request, &mut on_delta, &mut on_thinking)
                .await;
            // If the channel is gone the race is over; nothing to report.
            let _ = done_sender.send(RaceEvent::Done { index, result });
        });
        candidates.push(Candidate {
            index,
            started: Instant::now(),
            handle,
        });
    }

    /// Aborts every candidate except `winner`, returning how many were
    /// actually canceled (a finished loser needs no cancelation).
    fn cancel_losers(candidates: &mut Vec<Candidate>, winner: usize) -> u64 {
        let mut canceled = 0;
        candidates.retain(|candidate| {
            if candidate.index == winner {
                return true;
            }
            if !candidate.handle.is_finished() {
                candidate.handle.abort();
                canceled += 1;
            }
            false
        });
        canceled
    }
}

#[async_trait]
impl ModelProvider for HedgingProvider {
    fn name(&self) -> &str {
        self.primary.name()
    }

    fn model_id(&self) -> &str {
        self.primary.model_id()
    }

    fn context_window(&self) -> usize {
        self.primary.context_window()
    }

    fn capabilities(&self) -> crate::ModelCapabilities {
        self.primary.capabilities()
    }

    fn max_output_tokens(&self) -> Option<usize> {
        self.primary.max_output_tokens()
    }

    /// Non-streaming calls (context compression) are never hedged.
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.primary.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: ModelRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
        on_thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        stats::record_fast_request();
        let (tx, mut rx) = mpsc::unbounded_channel::<RaceEvent>();
        let mut candidates: Vec<Candidate> = Vec::new();
        Self::spawn_candidate(&mut candidates, self.primary.clone(), &request, &tx);

        let hedge_timer = tokio::time::sleep(self.threshold());
        tokio::pin!(hedge_timer);

        let mut hedged = false;
        let mut active = 1usize;
        let mut winner: Option<usize> = None;
        let mut winner_first_delta: Option<Instant> = None;
        let mut loser_chars = 0usize;
        let mut last_error: Option<ModelError> = None;
        let mut empty_response: Option<ModelResponse> = None;

        let result = loop {
            tokio::select! {
                () = &mut hedge_timer, if !hedged && winner.is_none() && self.config.max_parallel > 1 => {
                    self.hedge_now(&mut hedged, &mut active, &mut candidates, &request, &tx);
                }
                event = rx.recv() => {
                    let Some(event) = event else {
                        break last_words(
                            &mut empty_response,
                            last_error.take(),
                            "all hedged requests ended without a response",
                        );
                    };
                    match event {
                        RaceEvent::Delta { index, text, thinking } => {
                            if winner.is_none() && !text.is_empty() {
                                winner = Some(index);
                                winner_first_delta = Some(Instant::now());
                                stats::record_canceled(Self::cancel_losers(
                                    &mut candidates,
                                    index,
                                ));
                            }
                            if winner == Some(index) {
                                if thinking {
                                    on_thinking(text);
                                } else {
                                    on_delta(text);
                                }
                            } else {
                                loser_chars += text.len();
                            }
                        }
                        RaceEvent::Done { index, result: outcome } => {
                            active = active.saturating_sub(1);
                            match outcome {
                                Ok(response) => {
                                    match Self::settle(
                                        &mut candidates,
                                        index,
                                        winner,
                                        winner_first_delta,
                                        response,
                                    ) {
                                        Settled::Win(response) => break Ok(response),
                                        // A loser that finished before abort landed.
                                        Settled::Ignore => {}
                                        Settled::Degenerate(response) => {
                                            empty_response = Some(response);
                                            if active == 0 {
                                                break Ok(empty_response.take().unwrap_or_default());
                                            }
                                        }
                                    }
                                }
                                Err(error) => {
                                    if winner == Some(index) {
                                        break Err(error);
                                    }
                                    last_error = Some(error);
                                    // Failed fast before the threshold fired: hedge now
                                    // instead of waiting out the timer.
                                    if !hedged && winner.is_none() && self.config.max_parallel > 1 {
                                        self.hedge_now(&mut hedged, &mut active, &mut candidates, &request, &tx);
                                    } else if active == 0 {
                                        break last_words(
                                            &mut empty_response,
                                            last_error.take(),
                                            "all hedged requests failed",
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        };

        if hedged {
            stats::record_extra_tokens(estimate_tokens(loser_chars));
        }
        for candidate in &candidates {
            candidate.handle.abort();
        }
        result
    }
}

/// What a completed candidate means to the race.
enum Settled {
    /// This candidate wins; its metrics are already recorded.
    Win(ModelResponse),
    /// A loser that finished after the winner: ignore it.
    Ignore,
    /// No content and no tool calls: it cannot claim the win while others run,
    /// but it still beats a synthetic error if everything else fails.
    Degenerate(ModelResponse),
}

impl HedgingProvider {
    /// Classifies a completed candidate. A candidate that currently owns the
    /// stream wins, a late loser is ignored, and a degenerate response is held
    /// back as a fallback rather than accepted early.
    fn settle(
        candidates: &mut Vec<Candidate>,
        index: usize,
        winner: Option<usize>,
        winner_first_delta: Option<Instant>,
        response: ModelResponse,
    ) -> Settled {
        if winner == Some(index) {
            Self::claim_winner(candidates, index, winner_first_delta, &response);
            return Settled::Win(response);
        }
        if winner.is_some() {
            return Settled::Ignore;
        }
        if response.content.is_empty() && response.tool_calls.is_empty() {
            return Settled::Degenerate(response);
        }
        Self::claim_winner(candidates, index, winner_first_delta, &response);
        Settled::Win(response)
    }

    /// Starts the secondary request exactly once, whether the timer fired or the
    /// primary failed before the threshold.
    fn hedge_now(
        &self,
        hedged: &mut bool,
        active: &mut usize,
        candidates: &mut Vec<Candidate>,
        request: &ModelRequest,
        tx: &mpsc::UnboundedSender<RaceEvent>,
    ) {
        *hedged = true;
        *active += 1;
        stats::record_hedge_triggered(estimate_prompt_tokens(request));
        Self::spawn_candidate(candidates, self.secondary(), request, tx);
    }

    /// Cancels the losers and records the winner's metrics. Shared by the
    /// first-useful-content and useful-completion paths so the two cannot drift.
    fn claim_winner(
        candidates: &mut Vec<Candidate>,
        index: usize,
        winner_first_delta: Option<Instant>,
        response: &ModelResponse,
    ) {
        let canceled = Self::cancel_losers(candidates, index);
        stats::record_canceled(canceled);
        record_winner_metrics(candidates, index, winner_first_delta, response);
    }
}

/// The outcome once no candidate can produce anything better: a degenerate
/// response is still preferable to a synthetic error. Shared by both terminal
/// paths so their error text cannot drift.
fn last_words(
    empty: &mut Option<ModelResponse>,
    last_error: Option<ModelError>,
    fallback: &str,
) -> Result<ModelResponse, ModelError> {
    if let Some(empty) = empty.take() {
        return Ok(empty);
    }
    Err(last_error.unwrap_or_else(|| ModelError::InvalidResponse(fallback.to_owned())))
}

/// Rough chars/4 estimate; only used for cost accounting, never for billing.
fn estimate_tokens(chars: usize) -> u64 {
    (chars / 4) as u64
}

fn estimate_prompt_tokens(request: &ModelRequest) -> u64 {
    let mut chars = 0usize;
    for message in &request.messages {
        chars += message.content.len();
        for part in &message.parts {
            match part {
                ContentPart::Text { text } => chars += text.len(),
                ContentPart::Image { data, .. } => chars += data.len() / 8,
            }
        }
        for call in &message.tool_calls {
            chars += call.function.name.len() + call.function.arguments.len();
        }
    }
    for tool in &request.tools {
        chars += tool.function.name.len()
            + tool.function.description.len()
            + tool.function.parameters.to_string().len();
    }
    estimate_tokens(chars)
}

fn record_winner_metrics(
    candidates: &[Candidate],
    winner: usize,
    first_delta: Option<Instant>,
    response: &ModelResponse,
) {
    let Some(candidate) = candidates.iter().find(|c| c.index == winner) else {
        return;
    };
    let first_delta = first_delta.unwrap_or_else(Instant::now);
    let ttft = first_delta.saturating_duration_since(candidate.started);
    let output_chars = response.content.len()
        + response
            .tool_calls
            .iter()
            .map(|call| call.id.len() + call.function.name.len() + call.function.arguments.len())
            .sum::<usize>();
    let tokens = estimate_tokens(output_chars);
    let generated_in = candidate.started.elapsed().as_secs_f64();
    let tokens_per_sec = if generated_in > 0.0 {
        stats::to_f64(tokens) / generated_in
    } else {
        0.0
    };
    stats::record_winner(tokens, ttft, tokens_per_sec);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelCapabilities;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeProvider {
        name: &'static str,
        delay: Duration,
        deltas: Vec<String>,
        fail: bool,
        tool_call_only: bool,
        requests: Arc<AtomicUsize>,
    }

    impl FakeProvider {
        fn new(name: &'static str, delay_ms: u64, deltas: &[&str]) -> Arc<Self> {
            Arc::new(Self {
                name,
                delay: Duration::from_millis(delay_ms),
                deltas: deltas.iter().map(|s| (*s).to_owned()).collect(),
                fail: false,
                tool_call_only: false,
                requests: Arc::new(AtomicUsize::new(0)),
            })
        }

        fn failing(name: &'static str, delay_ms: u64) -> Arc<Self> {
            Arc::new(Self {
                name,
                delay: Duration::from_millis(delay_ms),
                deltas: Vec::new(),
                fail: true,
                tool_call_only: false,
                requests: Arc::new(AtomicUsize::new(0)),
            })
        }

        fn tool_calling(name: &'static str, delay_ms: u64) -> Arc<Self> {
            Arc::new(Self {
                name,
                delay: Duration::from_millis(delay_ms),
                deltas: Vec::new(),
                fail: false,
                tool_call_only: true,
                requests: Arc::new(AtomicUsize::new(0)),
            })
        }
    }

    #[async_trait]
    impl ModelProvider for FakeProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn model_id(&self) -> &'static str {
            "fake-model"
        }

        fn context_window(&self) -> usize {
            1_000
        }

        fn capabilities(&self) -> ModelCapabilities {
            ModelCapabilities::default()
        }

        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
            Err(ModelError::InvalidResponse("not used in tests".to_owned()))
        }

        async fn complete_stream(
            &self,
            _request: ModelRequest,
            on_delta: &mut (dyn FnMut(String) + Send),
            on_thinking: &mut (dyn FnMut(String) + Send),
        ) -> Result<ModelResponse, ModelError> {
            let _ = on_thinking;
            self.requests.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            if self.fail {
                return Err(ModelError::InvalidResponse(format!("{} failed", self.name)));
            }
            if self.tool_call_only {
                return Ok(ModelResponse {
                    usage: None,
                    content: String::new(),
                    tool_calls: vec![crate::ToolCall {
                        id: "call-1".to_owned(),
                        kind: "function".to_owned(),
                        function: crate::FunctionCall {
                            name: "shell".to_owned(),
                            arguments: "{}".to_owned(),
                        },
                    }],
                    finish_reason: Some("tool_calls".to_owned()),
                });
            }
            for delta in &self.deltas {
                on_delta(delta.clone());
            }
            Ok(ModelResponse {
                usage: None,
                content: self.deltas.concat(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            })
        }
    }

    fn hedge_config(threshold_ms: u64) -> HedgeConfig {
        HedgeConfig {
            threshold_override: Some(Duration::from_millis(threshold_ms)),
            max_parallel: 2,
        }
    }

    fn request() -> ModelRequest {
        ModelRequest {
            messages: vec![crate::Message::user("hi")],
            tools: Vec::new(),
        }
    }

    async fn race(provider: &HedgingProvider) -> (Result<ModelResponse, ModelError>, Vec<String>) {
        let mut forwarded = Vec::new();
        let mut thinking = Vec::new();
        let mut on_delta = |text: String| forwarded.push(text);
        let mut on_thinking = |text: String| thinking.push(text);
        let result = provider
            .complete_stream(request(), &mut on_delta, &mut on_thinking)
            .await;
        (result, forwarded)
    }

    #[tokio::test]
    async fn primary_wins_when_it_beats_the_threshold() {
        let primary = FakeProvider::new("primary", 5, &["fast"]);
        let secondary = FakeProvider::new("secondary", 5, &["unused"]);
        let secondary_requests = secondary.requests.clone();
        let provider = HedgingProvider::new(primary, vec![secondary], hedge_config(60_000));
        let (result, forwarded) = race(&provider).await;
        assert_eq!(result.unwrap().content, "fast");
        assert_eq!(forwarded, ["fast"]);
        assert_eq!(
            secondary_requests.load(Ordering::SeqCst),
            0,
            "secondary must not start when the primary answers in time"
        );
    }

    #[tokio::test]
    async fn slow_primary_triggers_secondary_and_first_delta_wins() {
        let primary = FakeProvider::new("primary", 500, &["slow"]);
        let secondary = FakeProvider::new("secondary", 10, &["hedged"]);
        let provider = HedgingProvider::new(primary, vec![secondary], hedge_config(20));
        let (result, forwarded) = race(&provider).await;
        assert_eq!(result.unwrap().content, "hedged");
        assert_eq!(forwarded, ["hedged"]);
    }

    #[tokio::test]
    async fn primary_failure_hedges_immediately_without_waiting() {
        let primary = FakeProvider::failing("primary", 5);
        let secondary = FakeProvider::new("secondary", 5, &["rescued"]);
        // A threshold far beyond the test runtime proves the error path
        // starts the secondary on its own.
        let provider = HedgingProvider::new(primary, vec![secondary], hedge_config(60_000));
        let (result, forwarded) = race(&provider).await;
        assert_eq!(result.unwrap().content, "rescued");
        assert_eq!(forwarded, ["rescued"]);
    }

    #[tokio::test]
    async fn all_candidates_failing_returns_the_error() {
        let primary = FakeProvider::failing("primary", 5);
        let secondary = FakeProvider::failing("secondary", 5);
        let provider = HedgingProvider::new(primary, vec![secondary], hedge_config(20));
        let (result, _) = race(&provider).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn without_alternates_the_same_provider_gets_a_second_request() {
        let primary = FakeProvider::new("solo", 400, &["solo"]);
        let requests = primary.requests.clone();
        let provider = HedgingProvider::new(primary, Vec::new(), hedge_config(20));
        let (result, _) = race(&provider).await;
        assert_eq!(result.unwrap().content, "solo");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn tool_call_only_response_can_win_without_deltas() {
        let primary = FakeProvider::tool_calling("primary", 10);
        let secondary = FakeProvider::new("secondary", 500, &["late"]);
        let provider = HedgingProvider::new(primary, vec![secondary], hedge_config(60_000));
        let (result, _) = race(&provider).await;
        let response = result.unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].function.name, "shell");
    }

    #[tokio::test]
    async fn max_parallel_one_disables_hedging() {
        let primary = FakeProvider::new("primary", 50, &["alone"]);
        let secondary = FakeProvider::new("secondary", 5, &["never"]);
        let secondary_requests = secondary.requests.clone();
        let config = HedgeConfig {
            threshold_override: Some(Duration::from_millis(1)),
            max_parallel: 1,
        };
        let provider = HedgingProvider::new(primary, vec![secondary], config);
        let (result, _) = race(&provider).await;
        assert_eq!(result.unwrap().content, "alone");
        assert_eq!(secondary_requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn prompt_estimate_counts_messages_and_tools() {
        let mut request = request();
        request.messages.push(crate::Message::assistant(
            "previous",
            vec![crate::ToolCall {
                id: "c".to_owned(),
                kind: "function".to_owned(),
                function: crate::FunctionCall {
                    name: "read".to_owned(),
                    arguments: "{\"path\":\"a\"}".to_owned(),
                },
            }],
        ));
        assert!(estimate_prompt_tokens(&request) > 0);
    }
}
