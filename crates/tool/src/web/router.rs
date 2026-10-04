//! Reachability/latency routing with bounded hedging and in-memory circuits.
use super::{
    SearchConfig, SearchProvider, SearchResult, ToolError, merge_completed,
    providers::{BochaSearch, BraveSearch, DuckDuckGoSearch, ProviderTimeout, SearxngSearch},
    telemetry,
};
use async_trait::async_trait;
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// One independently bounded candidate. `fallback` candidates always sort last.
pub struct SearchCandidate {
    pub name: String,
    pub provider: Arc<dyn SearchProvider>,
    pub timeout: Duration,
    pub fallback: bool,
}
#[derive(Clone, Debug, Default)]
pub struct ProviderStats {
    pub successes: u64,
    pub failures: u64,
    pub cancellations: u64,
    pub consecutive_failures: u32,
    pub latency: Option<Duration>,
    pub open_until: Option<Instant>,
}
pub struct SearchRouter {
    candidates: Vec<SearchCandidate>,
    stats: Mutex<Vec<ProviderStats>>,
    hedge_delay: Duration,
    failure_threshold: u32,
    cooldown: Duration,
}
impl SearchRouter {
    #[must_use]
    pub fn new(
        candidates: Vec<SearchCandidate>,
        hedge_delay: Duration,
        failure_threshold: u32,
        cooldown: Duration,
    ) -> Self {
        let stats = Mutex::new(vec![ProviderStats::default(); candidates.len()]);
        Self {
            candidates,
            stats,
            hedge_delay,
            failure_threshold: failure_threshold.max(1),
            cooldown,
        }
    }
    pub(super) fn configured(config: &SearchConfig, injected: Option<&reqwest::Client>) -> Self {
        let mut candidates = Vec::new();
        let client =
            |timeout: &ProviderTimeout| injected.cloned().unwrap_or_else(|| timeout.client());
        let mut add =
            |name: &str, provider: Arc<dyn SearchProvider>, timeout: &ProviderTimeout, fallback| {
                candidates.push(SearchCandidate {
                    name: name.into(),
                    provider,
                    timeout: timeout.total,
                    fallback,
                });
            };
        if let Some(key) = config
            .bocha_api_key
            .as_ref()
            .filter(|s| !s.trim().is_empty())
        {
            add(
                "bocha",
                Arc::new(BochaSearch {
                    client: client(&config.bocha_timeout),
                    url: config.bocha_url.clone(),
                    key: key.clone(),
                }),
                &config.bocha_timeout,
                false,
            );
        }
        if let Some(key) = config
            .brave_api_key
            .as_ref()
            .filter(|s| !s.trim().is_empty())
        {
            add(
                "brave",
                Arc::new(BraveSearch {
                    client: client(&config.brave_timeout),
                    url: config.brave_url.clone(),
                    key: key.clone(),
                }),
                &config.brave_timeout,
                false,
            );
        }
        if let Some(url) = config.searxng_url.as_ref().filter(|s| !s.trim().is_empty()) {
            add(
                "searxng",
                Arc::new(SearxngSearch {
                    client: client(&config.searxng_timeout),
                    url: url.clone(),
                }),
                &config.searxng_timeout,
                false,
            );
        }
        add(
            "duckduckgo",
            Arc::new(DuckDuckGoSearch {
                client: client(&config.duckduckgo_timeout),
                url: config.duckduckgo_url.clone(),
            }),
            &config.duckduckgo_timeout,
            true,
        );
        Self::new(
            candidates,
            config.hedge_delay,
            config.circuit_failure_threshold,
            config.circuit_cooldown,
        )
    }
    #[must_use]
    pub fn stats(&self) -> Vec<(String, ProviderStats)> {
        self.candidates
            .iter()
            .zip(
                self.stats
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter(),
            )
            .map(|(candidate, stats)| (candidate.name.clone(), stats.clone()))
            .collect()
    }
    fn eligible(&self, index: usize) -> bool {
        self.stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[index]
            .open_until
            .is_none_or(|until| until <= Instant::now())
    }
    fn order(&self) -> Vec<usize> {
        let stats = self
            .stats
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut order: Vec<_> = (0..self.candidates.len())
            .filter(|i| {
                stats[*i]
                    .open_until
                    .is_none_or(|until| until <= Instant::now())
            })
            .collect();
        order.sort_by_key(|i| {
            (
                self.candidates[*i].fallback,
                stats[*i].consecutive_failures,
                stats[*i].latency.unwrap_or(self.hedge_delay),
            )
        });
        order
    }
    async fn attempt(
        &self,
        index: usize,
        query: &str,
        limit: usize,
    ) -> (usize, Result<Vec<SearchResult>, ToolError>) {
        let candidate = &self.candidates[index];
        let mut guard = AttemptGuard {
            router: self,
            index,
            finished: false,
        };
        let start = Instant::now();
        let outcome =
            tokio::time::timeout(candidate.timeout, candidate.provider.search(query, limit))
                .await
                .unwrap_or_else(|_| {
                    Err(ToolError::Execution(format!(
                        "{}: search timeout",
                        candidate.name
                    )))
                });
        let elapsed = start.elapsed();
        telemetry::record(
            &format!("web.search.provider.{}.latency", candidate.name),
            elapsed,
        );
        telemetry::increment(&format!(
            "web.search.provider.{}.{}",
            candidate.name,
            if outcome.is_ok() {
                "success"
            } else {
                "failure"
            }
        ));
        {
            let mut stats = self
                .stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let history = &mut stats[index];
            // EWMA includes failed attempts, so slow unsuccessful providers rank lower.
            history.latency = Some(
                history
                    .latency
                    .map_or(elapsed, |old| old.mul_f64(0.75) + elapsed.mul_f64(0.25)),
            );
            if outcome.is_ok() {
                history.successes += 1;
                history.consecutive_failures = 0;
                history.open_until = None;
            } else {
                history.failures += 1;
                history.consecutive_failures += 1;
                if history.consecutive_failures >= self.failure_threshold {
                    history.open_until = Some(Instant::now() + self.cooldown);
                    telemetry::increment(&format!(
                        "web.search.provider.{}.circuit_open",
                        candidate.name
                    ));
                }
            }
        }
        guard.finished = true;
        (index, outcome)
    }
}
struct AttemptGuard<'a> {
    router: &'a SearchRouter,
    index: usize,
    finished: bool,
}
impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.router
                .stats
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[self.index]
                .cancellations += 1;
            telemetry::increment(&format!(
                "web.search.provider.{}.cancelled",
                self.router.candidates[self.index].name
            ));
        }
    }
}
#[async_trait]
impl SearchProvider for SearchRouter {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let limit = limit.clamp(1, 20);
        let mut order = self.order().into_iter();
        let mut pending = FuturesUnordered::new();
        let mut outcomes: Vec<Option<Result<Vec<SearchResult>, ToolError>>> =
            (0..self.candidates.len()).map(|_| None).collect();
        let queries = vec![query.to_owned(); self.candidates.len()];
        let mut next_launch = tokio::time::Instant::now();
        loop {
            // Launch one request initially or replace a completed failure/short response.
            if pending.is_empty() {
                let Some(index) = order.find(|i| self.eligible(*i)) else {
                    break;
                };
                pending.push(self.attempt(index, query, limit));
                next_launch = tokio::time::Instant::now() + self.hedge_delay;
            }
            tokio::select! {
                biased;
                completed = pending.next() => {
                    let Some((index, outcome)) = completed else { continue };
                    outcomes[index] = Some(outcome);
                    let mut merged = merge_completed(&queries, &outcomes);
                    if merged.len() >= limit {
                        merged.truncate(limit);
                        return Ok(merged);
                    }
                    // Advance on errors or short responses, with at most two in flight.
                    if let Some(index) = order.find(|i| self.eligible(*i)) {
                        telemetry::increment("web.search.fallback");
                        pending.push(self.attempt(index, query, limit));
                        next_launch = tokio::time::Instant::now() + self.hedge_delay;
                    }
                }
                () = tokio::time::sleep_until(next_launch), if pending.len() < 2 && order.len() > 0 => {
                    if let Some(index) = order.find(|i| self.eligible(*i)) {
                        telemetry::increment("web.search.hedged");
                        pending.push(self.attempt(index, query, limit));
                        next_launch = tokio::time::Instant::now() + self.hedge_delay;
                    }
                }
            }
        }
        let mut merged = merge_completed(&queries, &outcomes);
        merged.truncate(limit);
        if outcomes
            .iter()
            .any(|outcome| matches!(outcome, Some(Ok(_))))
        {
            return Ok(merged);
        }
        let errors = outcomes
            .iter()
            .enumerate()
            .filter_map(|(index, outcome)| {
                outcome
                    .as_ref()
                    .and_then(|r| r.as_ref().err())
                    .map(|e| format!("{}: {e}", self.candidates[index].name))
            })
            .collect::<Vec<_>>()
            .join("; ");
        Err(ToolError::Execution(format!(
            "search providers unavailable (circuits may be open): {errors}. Report provider errors and prefer fixing configuration or retrying later. Do not automatically bypass search providers with shell/Python scraping. User-requested alternative search or network diagnostics are allowed subject to tool permissions. Use web fetch for known URLs."
        )))
    }
}
