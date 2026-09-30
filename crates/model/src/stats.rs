//! Inference performance statistics: per-provider TTFT history, throughput,
//! error rates, and Fast-mode hedging metrics.
//!
//! The store is process-global and optionally persisted (via [`init`]) so the
//! adaptive hedge threshold learns across sessions. Recording happens once per
//! model request, never per delta, so persisting on every mutation is cheap.

use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{OnceLock, RwLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};

/// TTFT samples kept per provider/model; 64 is enough for a stable P95 while
/// still reacting to a provider that recently degraded.
const TTFT_WINDOW: usize = 64;
/// Winner TTFT samples kept for the `/status` P50/P95 display.
const WINNER_WINDOW: usize = 128;
/// Below this many samples the history is too noisy to derive a threshold.
const MIN_SAMPLES_FOR_THRESHOLD: usize = 8;
/// Cold-start hedge threshold before any history exists.
const DEFAULT_THRESHOLD: Duration = Duration::from_millis(1000);
/// Hedging earlier than this mostly burns tokens on requests that were fine.
const MIN_THRESHOLD: Duration = Duration::from_millis(200);
/// A request slower than this is a tail worth hedging regardless of history.
const MAX_THRESHOLD: Duration = Duration::from_millis(5000);
/// EMA weight for tokens/s updates.
const EMA_ALPHA: f64 = 0.3;

/// Historical performance of one provider serving one model.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProviderPerf {
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub errors: u64,
    /// Recent time-to-first-delta samples, oldest first.
    #[serde(default)]
    ttft_ms: VecDeque<u64>,
    /// EMA of observed generation throughput.
    #[serde(default)]
    pub tokens_per_sec: f64,
}

impl ProviderPerf {
    fn record_ttft(&mut self, duration: Duration) {
        self.ttft_ms.push_back(duration.as_millis() as u64);
        while self.ttft_ms.len() > TTFT_WINDOW {
            self.ttft_ms.pop_front();
        }
    }

    fn record_completion(&mut self, tokens_per_sec: f64) {
        if tokens_per_sec <= 0.0 {
            return;
        }
        self.tokens_per_sec = if self.tokens_per_sec <= 0.0 {
            tokens_per_sec
        } else {
            EMA_ALPHA.mul_add(tokens_per_sec, (1.0 - EMA_ALPHA) * self.tokens_per_sec)
        };
    }

    /// Share of requests that ended in a transport or HTTP error.
    #[must_use]
    pub fn error_rate(&self) -> f64 {
        let total = self.requests.max(1) as f64;
        (self.errors as f64 / total).min(1.0)
    }

    /// Nearest-rank percentile of recent TTFT samples, in milliseconds.
    #[must_use]
    pub fn ttft_percentile(&self, percentile: f64) -> Option<u64> {
        if self.ttft_ms.is_empty() {
            return None;
        }
        let mut sorted: Vec<u64> = self.ttft_ms.iter().copied().collect();
        sorted.sort_unstable();
        let rank = ((sorted.len() as f64) * percentile).ceil() as usize;
        Some(sorted[rank.max(1) - 1])
    }

    /// Median TTFT, used when ranking hedge candidates.
    #[must_use]
    pub fn ttft_p50(&self) -> Option<u64> {
        self.ttft_percentile(0.50)
    }

    /// Composite score for picking a hedge secondary; lower is better.
    ///
    /// Dominant term is typical TTFT; a 10% error rate adds 500 ms of
    /// penalty, and every 10 tokens/s of throughput subtracts 20 ms.
    /// Providers without history land on neutral defaults.
    #[must_use]
    pub fn hedge_score(&self) -> f64 {
        let ttft = self.ttft_p50().unwrap_or(1000) as f64;
        5000.0f64.mul_add(self.error_rate(), ttft) - 2.0 * self.tokens_per_sec
    }
}

/// Aggregated Fast-mode hedging metrics, shown by `/status`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HedgeStats {
    /// Streaming requests issued in Fast mode.
    #[serde(default)]
    pub fast_requests: u64,
    /// Requests where the secondary was actually started.
    #[serde(default)]
    pub hedged: u64,
    /// Loser requests canceled after a winner emerged.
    #[serde(default)]
    pub canceled: u64,
    /// Estimated tokens spent on requests that did not win (extra prompts of
    /// hedged secondaries plus whatever losers generated before cancelation).
    #[serde(default)]
    pub extra_tokens: u64,
    /// Estimated tokens in winner responses (the useful output).
    #[serde(default)]
    pub winner_tokens: u64,
    /// TTFT of the request that actually produced each response.
    #[serde(default)]
    winner_ttft_ms: VecDeque<u64>,
    /// EMA of winner generation throughput.
    #[serde(default)]
    pub tokens_per_sec: f64,
}

impl HedgeStats {
    fn record_winner(&mut self, tokens: u64, ttft: Duration, tokens_per_sec: f64) {
        self.winner_tokens += tokens;
        self.winner_ttft_ms.push_back(ttft.as_millis() as u64);
        while self.winner_ttft_ms.len() > WINNER_WINDOW {
            self.winner_ttft_ms.pop_front();
        }
        if tokens_per_sec > 0.0 {
            self.tokens_per_sec = if self.tokens_per_sec <= 0.0 {
                tokens_per_sec
            } else {
                EMA_ALPHA.mul_add(tokens_per_sec, (1.0 - EMA_ALPHA) * self.tokens_per_sec)
            };
        }
    }

    fn winner_ttft_percentile(&self, percentile: f64) -> Option<u64> {
        if self.winner_ttft_ms.is_empty() {
            return None;
        }
        let mut sorted: Vec<u64> = self.winner_ttft_ms.iter().copied().collect();
        sorted.sort_unstable();
        let rank = ((sorted.len() as f64) * percentile).ceil() as usize;
        Some(sorted[rank.max(1) - 1])
    }

    /// Share of Fast-mode requests that triggered a hedge.
    #[must_use]
    pub fn trigger_rate(&self) -> f64 {
        if self.fast_requests == 0 {
            0.0
        } else {
            self.hedged as f64 / self.fast_requests as f64
        }
    }

    /// Actual token cost relative to Standard mode, e.g. 1.14 means Fast
    /// mode cost 14% extra tokens. Never a marketing constant: 1.0 until
    /// hedging has actually spent extra tokens.
    #[must_use]
    pub fn extra_cost_factor(&self) -> f64 {
        if self.winner_tokens == 0 {
            return 1.0;
        }
        (self.winner_tokens + self.extra_tokens) as f64 / self.winner_tokens as f64
    }
}

/// Point-in-time copy of the hedging metrics for `/status`.
#[derive(Clone, Debug)]
pub struct HedgeSnapshot {
    pub fast_requests: u64,
    pub hedged: u64,
    pub canceled: u64,
    pub extra_tokens: u64,
    pub winner_tokens: u64,
    pub trigger_rate: f64,
    pub extra_cost_factor: f64,
    pub ttft_p50_ms: Option<u64>,
    pub ttft_p95_ms: Option<u64>,
    pub tokens_per_sec: f64,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    /// Keyed by `"provider/model"`.
    #[serde(default)]
    providers: BTreeMap<String, ProviderPerf>,
    #[serde(default)]
    hedge: HedgeStats,
}

static STORE: OnceLock<RwLock<Store>> = OnceLock::new();
static PERSIST_PATH: OnceLock<PathBuf> = OnceLock::new();

fn store() -> &'static RwLock<Store> {
    STORE.get_or_init(|| RwLock::new(Store::default()))
}

/// Enables persistence at `path` (the CLI passes
/// `~/.ax/inference_stats.json`) and loads any existing history.
pub fn init(path: PathBuf) {
    if let Ok(contents) = std::fs::read(&path)
        && let Ok(loaded) = serde_json::from_slice::<Store>(&contents)
        && let Ok(mut guard) = store().write()
    {
        *guard = loaded;
    }
    let _ = PERSIST_PATH.set(path);
}

/// Best-effort atomic persist; failures are ignored because stats are
/// advisory and must never break a model request.
fn persist(store: &Store) {
    let Some(path) = PERSIST_PATH.get() else {
        return;
    };
    let Ok(contents) = serde_json::to_vec(store) else {
        return;
    };
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, contents).is_ok() {
        let _ = std::fs::rename(&temporary, path);
    }
}

fn mutate<T>(update: impl FnOnce(&mut Store) -> T) -> T {
    let Ok(mut guard) = store().write() else {
        // A poisoned lock must not take inference down; drop the update.
        let mut fallback = Store::default();
        return update(&mut fallback);
    };
    let result = update(&mut guard);
    persist(&guard);
    result
}

fn key(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

/// Records the time to the first valid delta of a streaming request.
pub fn record_ttft(provider: &str, model: &str, duration: Duration) {
    let key = key(provider, model);
    mutate(|store| {
        let perf = store.providers.entry(key).or_default();
        perf.requests += 1;
        perf.record_ttft(duration);
    });
}

/// Records a finished stream: estimated output tokens over wall time.
pub fn record_completion(provider: &str, model: &str, output_tokens: u64, elapsed: Duration) {
    let seconds = elapsed.as_secs_f64();
    if seconds <= 0.0 {
        return;
    }
    let tokens_per_sec = output_tokens as f64 / seconds;
    let key = key(provider, model);
    mutate(|store| {
        store
            .providers
            .entry(key)
            .or_default()
            .record_completion(tokens_per_sec);
    });
}

/// Records a failed request for the provider's error rate.
pub fn record_error(provider: &str, model: &str) {
    let key = key(provider, model);
    mutate(|store| {
        let perf = store.providers.entry(key).or_default();
        perf.requests += 1;
        perf.errors += 1;
    });
}

/// A Fast-mode streaming request started.
pub fn record_fast_request() {
    mutate(|store| store.hedge.fast_requests += 1);
}

/// The hedge threshold fired and a secondary request was started. `extra_tokens`
/// is the estimated prompt cost of that duplicate request.
pub fn record_hedge_triggered(extra_tokens: u64) {
    mutate(|store| {
        store.hedge.hedged += 1;
        store.hedge.extra_tokens += extra_tokens;
    });
}

/// `count` loser requests were canceled after the winner emerged.
pub fn record_canceled(count: u64) {
    if count == 0 {
        return;
    }
    mutate(|store| store.hedge.canceled += count);
}

/// Adds estimated tokens generated by losers before they were canceled.
pub fn record_extra_tokens(tokens: u64) {
    if tokens == 0 {
        return;
    }
    mutate(|store| store.hedge.extra_tokens += tokens);
}

/// The winner completed: records its useful output, TTFT and throughput.
pub fn record_winner(tokens: u64, ttft: Duration, tokens_per_sec: f64) {
    mutate(|store| store.hedge.record_winner(tokens, ttft, tokens_per_sec));
}

/// The adaptive hedge threshold for a provider/model pair: the P95 of its
/// recent TTFT history, clamped to a sane range, or the cold-start default
/// until enough samples exist.
#[must_use]
pub fn hedge_threshold(provider: &str, model: &str) -> Duration {
    let Ok(guard) = store().read() else {
        return DEFAULT_THRESHOLD;
    };
    let Some(perf) = guard.providers.get(&key(provider, model)) else {
        return DEFAULT_THRESHOLD;
    };
    if perf.ttft_ms.len() < MIN_SAMPLES_FOR_THRESHOLD {
        return DEFAULT_THRESHOLD;
    }
    perf.ttft_percentile(0.95).map_or(DEFAULT_THRESHOLD, |p95| {
        Duration::from_millis(p95).clamp(MIN_THRESHOLD, MAX_THRESHOLD)
    })
}

/// A copy of one provider's performance record, for ranking hedge candidates.
#[must_use]
pub fn provider_perf(provider: &str, model: &str) -> ProviderPerf {
    store()
        .read()
        .ok()
        .and_then(|guard| guard.providers.get(&key(provider, model)).cloned())
        .unwrap_or_default()
}

/// A snapshot of the aggregated hedging metrics for `/status`.
#[must_use]
pub fn hedge_snapshot() -> HedgeSnapshot {
    let Ok(guard) = store().read() else {
        return HedgeSnapshot {
            fast_requests: 0,
            hedged: 0,
            canceled: 0,
            extra_tokens: 0,
            winner_tokens: 0,
            trigger_rate: 0.0,
            extra_cost_factor: 1.0,
            ttft_p50_ms: None,
            ttft_p95_ms: None,
            tokens_per_sec: 0.0,
        };
    };
    let hedge = &guard.hedge;
    HedgeSnapshot {
        fast_requests: hedge.fast_requests,
        hedged: hedge.hedged,
        canceled: hedge.canceled,
        extra_tokens: hedge.extra_tokens,
        winner_tokens: hedge.winner_tokens,
        trigger_rate: hedge.trigger_rate(),
        extra_cost_factor: hedge.extra_cost_factor(),
        ttft_p50_ms: hedge.winner_ttft_percentile(0.50),
        ttft_p95_ms: hedge.winner_ttft_percentile(0.95),
        tokens_per_sec: hedge.tokens_per_sec,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_start_threshold_is_default() {
        let store = Store::default();
        assert!(store.providers.get("none/model").is_none());
        assert_eq!(DEFAULT_THRESHOLD, Duration::from_millis(1000));
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let mut perf = ProviderPerf::default();
        for sample in [100, 200, 300, 400, 500, 600, 700, 800, 900, 1000] {
            perf.record_ttft(Duration::from_millis(sample));
        }
        assert_eq!(perf.ttft_p50(), Some(500));
        assert_eq!(perf.ttft_percentile(0.95), Some(1000));
    }

    #[test]
    fn threshold_window_discards_oldest_samples() {
        let mut perf = ProviderPerf::default();
        for _ in 0..(TTFT_WINDOW + 10) {
            perf.record_ttft(Duration::from_millis(50));
        }
        assert_eq!(perf.ttft_ms.len(), TTFT_WINDOW);
    }

    #[test]
    fn error_rate_is_bounded() {
        let mut perf = ProviderPerf::default();
        perf.requests = 4;
        perf.errors = 2;
        assert!((perf.error_rate() - 0.5).abs() < f64::EPSILON);
        perf.requests = 0;
        perf.errors = 0;
        assert_eq!(perf.error_rate(), 0.0);
    }

    #[test]
    fn extra_cost_factor_reflects_actual_spend() {
        let mut hedge = HedgeStats::default();
        assert_eq!(hedge.extra_cost_factor(), 1.0);
        hedge.winner_tokens = 10_000;
        assert_eq!(hedge.extra_cost_factor(), 1.0);
        hedge.extra_tokens = 1_400;
        assert!((hedge.extra_cost_factor() - 1.14).abs() < 1e-9);
    }

    #[test]
    fn trigger_rate_uses_fast_requests_as_base() {
        let mut hedge = HedgeStats::default();
        assert_eq!(hedge.trigger_rate(), 0.0);
        hedge.fast_requests = 24;
        hedge.hedged = 3;
        assert!((hedge.trigger_rate() - 0.125).abs() < 1e-9);
    }

    #[test]
    fn throughput_uses_ema() {
        let mut perf = ProviderPerf::default();
        perf.record_completion(100.0);
        assert_eq!(perf.tokens_per_sec, 100.0);
        perf.record_completion(0.0);
        assert_eq!(perf.tokens_per_sec, 100.0, "zero-rate samples are ignored");
        perf.record_completion(50.0);
        assert!(perf.tokens_per_sec < 100.0 && perf.tokens_per_sec > 50.0);
    }

    #[test]
    fn hedge_score_prefers_fast_reliable_providers() {
        let mut slow = ProviderPerf::default();
        for _ in 0..10 {
            slow.record_ttft(Duration::from_millis(900));
        }
        let mut fast = ProviderPerf::default();
        for _ in 0..10 {
            fast.record_ttft(Duration::from_millis(200));
        }
        fast.record_completion(80.0);
        assert!(fast.hedge_score() < slow.hedge_score());

        let mut flaky = fast.clone();
        flaky.requests = 10;
        flaky.errors = 5;
        assert!(flaky.hedge_score() > fast.hedge_score());
    }

    #[test]
    fn serde_round_trip_preserves_history() {
        let mut store = Store::default();
        let perf = store.providers.entry("p/m".to_owned()).or_default();
        perf.record_ttft(Duration::from_millis(321));
        perf.requests = 1;
        store.hedge.fast_requests = 2;
        store.hedge.extra_tokens = 42;
        let encoded = serde_json::to_vec(&store).unwrap();
        let decoded: Store = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.providers["p/m"].ttft_p50(), Some(321));
        assert_eq!(decoded.hedge.extra_tokens, 42);
    }
}
