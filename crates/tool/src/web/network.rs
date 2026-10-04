//! Shared transport, bounded retries, DNS caching and real phase measurements.
use super::{FetchError, FetchErrorKind, MAX_BYTES, ToolError, telemetry};
use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::OnceCell;
use tower::{Layer, Service};

const DNS_TTL: Duration = Duration::from_secs(60);
const DNS_CAPACITY: usize = 256;
const RETRY_DELAY: Duration = Duration::from_millis(100);
pub(super) const FETCH_CONCURRENCY: usize = 3;

pub(super) fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .read_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(8))
                .redirect(reqwest::redirect::Policy::limited(5))
                .pool_idle_timeout(Duration::from_secs(90))
                .pool_max_idle_per_host(6)
                .tcp_keepalive(Duration::from_secs(30))
                .dns_resolver(Arc::new(CachedDns::default()))
                .connector_layer(ConnectTimingLayer)
                .user_agent(concat!("AX/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("valid web client")
        })
        .clone()
}

/// Search uses independently configured clients; fetch transport is unchanged.
pub(super) fn search_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(6)
        .tcp_keepalive(Duration::from_secs(30))
        .dns_resolver(Arc::new(CachedDns::default()))
        .connector_layer(ConnectTimingLayer)
        .user_agent(concat!("AX/", env!("CARGO_PKG_VERSION")))
}
/// Routing replaces retries for search, including POST. No detached tasks.
pub(super) async fn search_request(
    request: reqwest::RequestBuilder,
    url: &str,
    text_only: bool,
) -> Result<Body, FetchError> {
    let _timer = telemetry::Timer::new("web.http.total");
    get_once(request, url, text_only).await
}

type DnsEntry = Arc<OnceCell<(Instant, Vec<SocketAddr>)>>;

#[derive(Default)]
struct CachedDns {
    entries: Arc<Mutex<HashMap<String, DnsEntry>>>,
}

impl Resolve for CachedDns {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let entry = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            entries.retain(|_, cell| cell.get().is_none_or(|(at, _)| at.elapsed() < DNS_TTL));
            if entries.len() >= DNS_CAPACITY && !entries.contains_key(&host) {
                // The cache is bounded even when a turn visits many hosts.
                if let Some(oldest) = entries.keys().next().cloned() {
                    entries.remove(&oldest);
                }
            }
            Arc::clone(entries.entry(host.clone()).or_default())
        };
        Box::pin(async move {
            if entry.get().is_some() {
                telemetry::increment("web.http.dns_cache_hit");
            }
            let (_, addresses) = entry
                .get_or_try_init(|| async {
                    let _timer = telemetry::Timer::new("web.http.dns");
                    let addresses = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
                    Ok::<_, std::io::Error>((Instant::now(), addresses))
                })
                .await?;
            Ok(Box::new(addresses.clone().into_iter()) as Addrs)
        })
    }
}

#[derive(Clone)]
struct ConnectTimingLayer;

impl<S> Layer<S> for ConnectTimingLayer {
    type Service = ConnectTiming<S>;
    fn layer(&self, inner: S) -> Self::Service {
        ConnectTiming { inner }
    }
}

#[derive(Clone)]
struct ConnectTiming<S> {
    inner: S,
}

impl<S, Request> Service<Request> for ConnectTiming<S>
where
    S: Service<Request>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }
    fn call(&mut self, request: Request) -> Self::Future {
        let timer = telemetry::Timer::new("web.http.connect");
        let future = self.inner.call(request);
        Box::pin(async move {
            let _timer = timer;
            future.await
        })
    }
}

pub(super) struct Body {
    pub url: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Every request has at most two attempts. Dropping this future cancels reads,
/// retry backoff and further attempts; there are no detached request tasks.
pub(super) async fn get(
    request: reqwest::RequestBuilder,
    url: &str,
    text_only: bool,
) -> Result<Body, FetchError> {
    let _timer = telemetry::Timer::new("web.http.total");
    for attempt in 0..2 {
        let Some(request) = request.try_clone() else {
            return Err(policy_error(url, "web GET request could not be cloned"));
        };
        let outcome = get_once(request, url, text_only).await;
        match outcome {
            Err(error) if attempt == 0 && retryable(&error) => {
                telemetry::increment("web.http.retry");
                tokio::time::sleep(RETRY_DELAY).await;
            }
            other => return other,
        }
    }
    unreachable!("the final attempt always returns")
}

fn retryable(error: &FetchError) -> bool {
    error.kind == FetchErrorKind::Timeout
        || error
            .status_code
            .is_some_and(|code| code == 429 || (500..600).contains(&code))
}

fn policy_error(url: &str, message: &str) -> FetchError {
    FetchError::from_error(url, &ToolError::Execution(message.into()))
}

async fn get_once(
    request: reqwest::RequestBuilder,
    url: &str,
    text_only: bool,
) -> Result<Body, FetchError> {
    let started = Instant::now();
    let response = request
        .send()
        .await
        .map_err(|error| FetchError::from_error(url, &error))?;
    // reqwest exposes receipt of response headers, not the first socket byte.
    telemetry::record("web.http.ttfb", started.elapsed());
    let response = response
        .error_for_status()
        .map_err(|error| FetchError::from_error(url, &error))?;
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if text_only
        && !(content_type.starts_with("text/html")
            || content_type.starts_with("text/plain")
            || content_type.starts_with("text/markdown"))
    {
        return Err(policy_error(
            url,
            &format!("unsupported content type: {content_type}"),
        ));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(policy_error(url, "web response exceeds size limit"));
    }
    let final_url = response.url().to_string();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| FetchError::from_error(url, &error))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err(policy_error(url, "web response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(Body {
        url: final_url,
        content_type,
        bytes,
    })
}

pub(super) fn policy_client(
    profiles: Vec<crate::PermissionProfile>,
) -> Result<reqwest::Client, ToolError> {
    policy_client_builder(profiles)
        .build()
        .map_err(|e| ToolError::Execution(e.to_string()))
}

pub(super) fn policy_client_builder(
    profiles: Vec<crate::PermissionProfile>,
) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .read_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("redirect limit");
            }
            if profiles.iter().any(|p| {
                matches!(
                    p.network_decision(attempt.url()),
                    Some(crate::PermissionDecision::Deny | crate::PermissionDecision::Ask)
                )
            }) {
                return attempt.error("redirect target requires separate permission");
            }
            attempt.follow()
        }))
        .dns_resolver(Arc::new(CachedDns::default()))
        .user_agent(concat!("AX/", env!("CARGO_PKG_VERSION")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[tokio::test]
    async fn dns_cache_coalesces_and_reuses_successful_lookups() {
        let dns = CachedDns::default();
        let name = || Name::from_str("localhost").unwrap();
        let (first, second) = tokio::join!(dns.resolve(name()), dns.resolve(name()));
        assert!(!first.unwrap().collect::<Vec<_>>().is_empty());
        assert!(!second.unwrap().collect::<Vec<_>>().is_empty());
        let entries = dns.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries["localhost"].get().is_some());
    }

    #[test]
    fn retries_are_limited_to_timeout_rate_limit_and_server_errors() {
        for code in [401, 403, 404] {
            let mut error = policy_error("https://example.test", "status");
            error.status_code = Some(code);
            assert!(!retryable(&error));
        }
        for code in [429, 500, 503] {
            let mut error = policy_error("https://example.test", "status");
            error.status_code = Some(code);
            assert!(retryable(&error));
        }
    }
}
