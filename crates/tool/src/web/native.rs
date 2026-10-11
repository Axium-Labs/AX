//! Sequential native-first routing bound to the selected model adapter.
use super::{SearchProvider, SearchResult, ToolError, telemetry};
use async_trait::async_trait;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Health {
    verified: bool,
    unsupported: bool,
    failures: u32,
    open_until: Option<Instant>,
}
#[derive(Default)]
pub(super) struct NativeState {
    health: Mutex<Health>,
    probe: tokio::sync::Mutex<()>,
}
pub(super) struct NativeFirst {
    pub model: Option<Arc<dyn model::ModelProvider>>,
    pub remote: Arc<dyn SearchProvider>,
    pub state: Arc<NativeState>,
    pub native_timeout: Duration,
    pub task_timeout: Duration,
}
impl NativeFirst {
    fn eligible(&self) -> bool {
        let state = self
            .state
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !state.unsupported && state.open_until.is_none_or(|until| until <= Instant::now())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One native entitlement attempt and its remote fallback share a task budget"
    )]
    async fn run(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let mut native_error = None;
        if let Some(provider) = &self.model
            && let Some(protocol) = provider.native_search_protocol()
        {
            let verified = self
                .state
                .health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .verified;
            // Serialize initial entitlement probes. Distinct searches can run
            // concurrently once this concrete account/model has succeeded.
            let mut probe = if verified {
                None
            } else {
                Some(self.state.probe.lock().await)
            };
            if self
                .state
                .health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .verified
            {
                probe.take();
            }
            if self.eligible() {
                let _timer = telemetry::Timer::new("web.search.native");
                let outcome = tokio::time::timeout(
                    self.native_timeout.min(self.task_timeout / 2),
                    provider.native_search(query),
                )
                .await;
                match outcome {
                    Ok(Ok(response))
                        if !response.sources.is_empty()
                            && response.sources.iter().all(|s| {
                                reqwest::Url::parse(&s.url)
                                    .is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
                            }) =>
                    {
                        {
                            let mut state = self
                                .state
                                .health
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            state.verified = true;
                            state.failures = 0;
                            state.open_until = None;
                        }
                        telemetry::increment("web.search.native.success");
                        let citations = serde_json::to_value(&response.sources).unwrap_or_default();
                        let mut results = response
                            .sources
                            .into_iter()
                            .enumerate()
                            .map(|(index, s)| {
                                // Keep the full answer once in evidence rather than
                                // multiplying its tokens by the number of sources.
                                let snippet = if index == 0 {
                                    response.answer.as_str()
                                } else {
                                    s.citation["cited_text"].as_str().unwrap_or_default()
                                };
                                let mut result = SearchResult::new(
                                    s.title,
                                    s.url,
                                    snippet.chars().take(2000).collect::<String>(),
                                    format!("native:{}", provider.name()),
                                );
                                result.evidence.push(
                                    serde_json::json!({"citation":s.citation,"protocol":protocol}),
                                );
                                result
                            })
                            .collect::<Vec<_>>();
                        // Never discard platform citation offsets/suggestions,
                        // server tool results or usage when normalizing URLs.
                        results[0].evidence.push(serde_json::json!({"native_answer":response.answer,"provider_evidence":response.evidence,"sources":citations}));
                        return Ok(results);
                    }
                    other => {
                        let (status, retry_after) = match &other {
                            Ok(Err(model::ModelError::HttpResponse {
                                status,
                                retry_after,
                                ..
                            })) => (Some(*status), *retry_after),
                            Ok(Err(model::ModelError::HttpStatus { status, .. })) => {
                                (Some(*status), None)
                            }
                            _ => (None, None),
                        };
                        // Bodies and tokens never enter tool errors or model context.
                        native_error = Some(status.map_or_else(
                            || "native search unavailable, invalid response or timed out".into(),
                            |s| format!("native search HTTP {s}"),
                        ));
                        let mut state = self
                            .state
                            .health
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        state.failures += 1;
                        state.unsupported = status.is_some_and(|s| matches!(s, 400 | 404 | 422));
                        if !state.verified
                            || state.failures >= 3
                            || status.is_some_and(|s| matches!(s, 401 | 403 | 429))
                        {
                            state.open_until = Instant::now()
                                .checked_add(retry_after.unwrap_or(Duration::from_secs(30)));
                        }
                        telemetry::increment("web.search.native.failure");
                    }
                }
            }
            drop(probe);
        }
        telemetry::increment("web.search.remote");
        match self.remote.search(query, limit).await {
            Ok(mut results) => {
                if let (Some(error), Some(result)) = (native_error, results.first_mut()) {
                    result
                        .evidence
                        .push(serde_json::json!({"native_fallback":error}));
                }
                Ok(results)
            }
            Err(error) => Err(ToolError::Execution(format!(
                "{}; remote: {error}",
                native_error.unwrap_or_else(|| "native search unsupported or cooling down".into())
            ))),
        }
    }
}
#[async_trait]
impl SearchProvider for NativeFirst {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        tokio::time::timeout(self.task_timeout, self.run(query, limit))
            .await
            .unwrap_or_else(|_| {
                Err(ToolError::Execution(
                    "web search task time budget exhausted".into(),
                ))
            })
    }
}
