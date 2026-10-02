//! Bounded retry policy shared by runtime callers. No semantic string matching.
use crate::ModelError;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    Terminal,
    Retryable,
}
fn transient_io(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
    )
}
/// Statuses that mean "the same request may succeed later".
///
/// `408` (request timeout) and `425` (too early) are 4xx but explicitly ask the
/// client to try again. The rest of the 4xx range is a client-side fault that a
/// retry cannot fix.
#[must_use]
pub fn status_retryable(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || (500..600).contains(&status)
}
fn transport_retryable(error: &reqwest::Error) -> bool {
    if let Some(status) = error.status() {
        return status_retryable(status.as_u16());
    }
    if error.is_timeout() || error.is_connect() || error.is_body() {
        return true;
    }
    let mut source = std::error::Error::source(error);
    while let Some(error) = source {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| transient_io(e.kind()))
        {
            return true;
        }
        source = error.source();
    }
    false
}
impl ModelError {
    #[must_use]
    pub fn error_class(&self) -> ErrorClass {
        match self {
            Self::HttpStatus { status, .. } | Self::HttpResponse { status, .. }
                if status_retryable(*status) =>
            {
                ErrorClass::Retryable
            }
            Self::Transport(e) if transport_retryable(e) => ErrorClass::Retryable,
            Self::Io(e) if transient_io(e.kind()) => ErrorClass::Retryable,
            _ => ErrorClass::Terminal,
        }
    }
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::HttpResponse { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryPolicy {
    /// Includes the initial attempt.
    pub max_attempts: usize,
    pub time_budget_ms: u64,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            time_budget_ms: 30_000,
            base_delay_ms: 200,
            max_delay_ms: 5_000,
        }
    }
}
impl RetryPolicy {
    /// Retry-After is never capped below the server's requested wait. A wait
    /// outside the remaining time budget stops retrying instead.
    #[must_use]
    pub fn delay(
        &self,
        error: &ModelError,
        attempts: usize,
        elapsed: Duration,
        entropy: u64,
    ) -> Option<Duration> {
        if error.error_class() != ErrorClass::Retryable || attempts >= self.max_attempts {
            return None;
        }
        let cap = self
            .base_delay_ms
            .saturating_mul(
                1u64.checked_shl(u32::try_from(attempts.saturating_sub(1)).unwrap_or(u32::MAX))
                    .unwrap_or(u64::MAX),
            )
            .min(self.max_delay_ms);
        let delay = error
            .retry_after()
            .unwrap_or_else(|| Duration::from_millis(entropy % cap.saturating_add(1).max(1)));
        (elapsed.saturating_add(delay) < Duration::from_millis(self.time_budget_ms))
            .then_some(delay)
    }
}
pub fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    value
        .parse::<u64>()
        .map(Duration::from_secs)
        .ok()
        .or_else(|| {
            httpdate::parse_http_date(value)
                .ok()
                .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn status(status: u16) -> ModelError {
        ModelError::HttpStatus {
            status,
            message: String::new(),
        }
    }
    #[test]
    fn classify_and_budgets() {
        for code in [400, 401, 403, 404, 422] {
            assert_eq!(status(code).error_class(), ErrorClass::Terminal);
        }
        for code in [408, 425, 429, 500, 503] {
            assert_eq!(status(code).error_class(), ErrorClass::Retryable);
        }
        assert!(!status_retryable(400));
        assert!(status_retryable(408));
        assert!(status_retryable(425));
        let policy = RetryPolicy::default();
        assert!(policy.delay(&status(400), 1, Duration::ZERO, 1).is_none());
        assert!(policy.delay(&status(503), 4, Duration::ZERO, 1).is_none());
        assert!(
            policy
                .delay(&status(503), 1, Duration::from_secs(30), 1)
                .is_none()
        );
        let error = ModelError::HttpResponse {
            status: 429,
            message: String::new(),
            retry_after: Some(Duration::from_secs(10)),
        };
        assert_eq!(
            policy.delay(&error, 1, Duration::ZERO, 1),
            Some(Duration::from_secs(10))
        );
        assert!(
            policy
                .delay(&error, 1, Duration::from_secs(21), 1)
                .is_none()
        );
    }
    #[test]
    fn reset_is_retryable_but_configuration_and_bad_response_are_terminal() {
        assert_eq!(
            ModelError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)).error_class(),
            ErrorClass::Retryable
        );
        assert_eq!(
            ModelError::Configuration("bad".into()).error_class(),
            ErrorClass::Terminal
        );
        assert_eq!(
            ModelError::InvalidResponse("bad".into()).error_class(),
            ErrorClass::Terminal
        );
        let policy = RetryPolicy::default();
        let error = status(503);
        let a = policy.delay(&error, 2, Duration::ZERO, 1).unwrap();
        let b = policy.delay(&error, 2, Duration::ZERO, 31).unwrap();
        assert_ne!(a, b);
        assert!(b <= Duration::from_millis(policy.max_delay_ms));
    }
    #[test]
    fn header_seconds_and_date() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(retry_after_header(&headers), Some(Duration::from_secs(7)));
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert_eq!(retry_after_header(&headers), Some(Duration::ZERO));
    }
}
