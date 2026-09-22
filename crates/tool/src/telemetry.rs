//! Bounded, process-local latency metrics. No prompts, arguments, or credentials.
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Default)]
pub struct Metric {
    pub count: u64,
    pub total_micros: u128,
    pub max_micros: u128,
}
static METRICS: OnceLock<Mutex<BTreeMap<String, Metric>>> = OnceLock::new();
pub struct Timer {
    label: String,
    start: Instant,
}
impl Timer {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            start: Instant::now(),
        }
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        record(&self.label, self.start.elapsed());
    }
}
pub fn record(label: &str, elapsed: Duration) {
    if let Ok(mut metrics) = METRICS.get_or_init(|| Mutex::new(BTreeMap::new())).lock() {
        if metrics.len() >= 256 && !metrics.contains_key(label) {
            return;
        }
        let entry = metrics.entry(label.to_owned()).or_default();
        entry.count += 1;
        entry.total_micros += elapsed.as_micros();
        entry.max_micros = entry.max_micros.max(elapsed.as_micros());
    }
}
pub fn snapshot() -> BTreeMap<String, Metric> {
    METRICS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map(|m| m.clone())
        .unwrap_or_default()
}
