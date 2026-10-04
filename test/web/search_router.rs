use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tool::{
    SearchCandidate, SearchConfig, SearchProvider, SearchResult, SearchRouter, Tool, ToolError,
    WebTool,
};
struct Mock {
    delay: Duration,
    error: Option<&'static str>,
    urls: Vec<&'static str>,
    started: Arc<AtomicUsize>,
    completed: Arc<AtomicUsize>,
}
#[async_trait]
impl SearchProvider for Mock {
    async fn search(&self, _: &str, _: usize) -> Result<Vec<SearchResult>, ToolError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.completed.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = self.error {
            Err(ToolError::Execution(e.into()))
        } else {
            Ok(self
                .urls
                .iter()
                .map(|url| SearchResult::new("title", *url, "snippet", "mock"))
                .collect())
        }
    }
}
fn candidate(
    name: &str,
    delay: u64,
    error: Option<&'static str>,
    urls: Vec<&'static str>,
) -> (SearchCandidate, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let started = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    (
        SearchCandidate {
            name: name.into(),
            timeout: Duration::from_secs(1),
            fallback: false,
            provider: Arc::new(Mock {
                delay: Duration::from_millis(delay),
                error,
                urls,
                started: started.clone(),
                completed: completed.clone(),
            }),
        },
        started,
        completed,
    )
}
fn router(candidates: Vec<SearchCandidate>) -> SearchRouter {
    SearchRouter::new(
        candidates,
        Duration::from_millis(30),
        2,
        Duration::from_millis(60),
    )
}
#[tokio::test]
async fn fallback_for_transport_errors() {
    for error in [
        "DNS lookup failed",
        "connection refused",
        "429",
        "503",
        "challenge",
    ] {
        let (a, _, _) = candidate("a", 0, Some(error), vec![]);
        let (b, hits, _) = candidate("b", 0, None, vec!["https://example.test/a"]);
        let router = router(vec![a, b]);
        assert_eq!(router.search("q", 1).await.unwrap().len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(router.stats()[0].1.failures, 1);
    }
}
#[tokio::test]
async fn timeout_falls_back_and_cancels_timed_out_future() {
    let (mut a, _, completed) = candidate("slow", 300, None, vec!["https://example.test/slow"]);
    a.timeout = Duration::from_millis(10);
    let (b, _, _) = candidate("fast", 0, None, vec!["https://example.test/fast"]);
    let router = router(vec![a, b]);
    assert_eq!(
        router.search("q", 1).await.unwrap()[0].url,
        "https://example.test/fast"
    );
    assert_eq!(router.stats()[0].1.failures, 1);
    tokio::time::sleep(Duration::from_millis(320)).await;
    assert_eq!(completed.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn hedged_winner_cancels_loser_without_starting_all_candidates() {
    let (a, hits, completed) = candidate("slow", 300, None, vec!["https://example.test/slow"]);
    let (b, other, _) = candidate("fast", 0, None, vec!["https://example.test/fast"]);
    let (c, third, _) = candidate("third", 0, None, vec!["https://example.test/third"]);
    let router = router(vec![a, b, c]);
    let start = std::time::Instant::now();
    assert_eq!(
        router.search("q", 1).await.unwrap()[0].url,
        "https://example.test/fast"
    );
    assert!(start.elapsed() >= Duration::from_millis(30));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(other.load(Ordering::SeqCst), 1);
    assert_eq!(third.load(Ordering::SeqCst), 0);
    assert_eq!(router.stats()[0].1.cancellations, 1);
    assert_eq!(router.stats()[0].1.failures, 0);
    tokio::time::sleep(Duration::from_millis(320)).await;
    assert_eq!(completed.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn fast_primary_does_not_launch_paid_hedge() {
    let (a, _, _) = candidate("a", 0, None, vec!["https://example.test/a"]);
    let (b, hits, _) = candidate("b", 0, None, vec!["https://example.test/b"]);
    let router = router(vec![a, b]);
    router.search("q", 1).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn circuit_skips_then_allows_retry_after_cooldown() {
    let (a, hits, _) = candidate("a", 0, Some("503"), vec![]);
    let router = router(vec![a]);
    assert!(router.search("q", 1).await.is_err());
    assert!(router.search("q", 1).await.is_err());
    assert!(router.stats()[0].1.open_until.is_some());
    assert!(router.search("q", 1).await.is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(router.search("q", 1).await.is_err());
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn merge_dedup_limit_across_providers() {
    let (a, _, _) = candidate(
        "a",
        0,
        None,
        vec![
            "https://EXAMPLE.test/a/?utm_source=x#fragment",
            "https://example.test/a",
        ],
    );
    let (b, hits, _) = candidate(
        "b",
        0,
        None,
        vec![
            "https://example.test/a#other",
            "https://example.test/b",
            "https://example.test/c",
        ],
    );
    let router = router(vec![a, b]);
    let results = router.search("q", 2).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[1].url.ends_with("/b"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn ranking_uses_observed_latency_and_fallback_stays_last() {
    let (a, hits, _) = candidate("a", 15, None, vec!["https://example.test/a"]);
    let (b, other, _) = candidate("b", 0, None, vec!["https://example.test/b"]);
    let (mut c, last, _) = candidate("ddg", 0, None, vec!["https://example.test/c"]);
    c.fallback = true;
    let router = router(vec![a, b, c]);
    router.search("q", 2).await.unwrap();
    let previous = hits.load(Ordering::SeqCst);
    router.search("q", 1).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), previous);
    assert_eq!(other.load(Ordering::SeqCst), 2);
    assert_eq!(last.load(Ordering::SeqCst), 0);
}
struct Server {
    url: String,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(status: u16, body: &str) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let body = body.to_owned();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let body = body.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut buffer = [0; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                counter.fetch_add(1, Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 {status} Response\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    Server { url, hits, task }
}
const DDG: &str = r#"<div class="result"><a class="result__a" href="https://example.test/ddg">Title &amp; More</a><span class="result__snippet">body</span></div>"#;
fn web(config: SearchConfig) -> WebTool {
    WebTool::new()
        .with_client(reqwest::Client::builder().no_proxy().build().unwrap())
        .with_search_config(config)
}
#[tokio::test]
async fn actual_http_status_challenge_and_invalid_json_fallback() {
    for (status, body) in [
        (429, ""),
        (503, ""),
        (200, "<form>captcha challenge</form>"),
        (200, r#"{"error":"challenge"}"#),
    ] {
        let primary = server(status, body).await;
        let fallback = server(200, DDG).await;
        let tool = web(SearchConfig {
            brave_api_key: Some("fixture".into()),
            brave_url: primary.url.clone(),
            duckduckgo_url: fallback.url.clone(),
            ..Default::default()
        });
        let value: Value = serde_json::from_str(
            &tool
                .execute(json!({"operation":"search","query":"q","limit":1}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["results"][0]["source"], "duckduckgo");
        assert_eq!(primary.hits.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn bocha_and_searxng_normalize_and_respect_configuration() {
    for (body, source) in [
        (
            r#"{"code":200,"data":{"webPages":{"value":[{"name":"Title","url":"https://example.test/a","snippet":"text"}]}}}"#,
            "bocha",
        ),
        (
            r#"{"results":[{"title":"Title","url":"https://example.test/a","content":"text"}]}"#,
            "searxng",
        ),
    ] {
        let primary = server(200, body).await;
        let ddg = server(200, DDG).await;
        let mut config = SearchConfig {
            duckduckgo_url: ddg.url.clone(),
            ..Default::default()
        };
        if source == "bocha" {
            config.bocha_api_key = Some("key".into());
            config.bocha_url = primary.url.clone();
        } else {
            config.searxng_url = Some(primary.url.clone());
        }
        let value: Value = serde_json::from_str(
            &web(config)
                .execute(json!({"operation":"search","query":"q","limit":1}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["results"][0]["source"], source);
        assert_eq!(value["results"][0]["snippet"], "text");
        assert_eq!(ddg.hits.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn all_failed_search_guidance_allows_explicit_user_alternatives() {
    let ddg = server(200, "<form>challenge</form>").await;
    let error = web(SearchConfig {
        duckduckgo_url: ddg.url.clone(),
        ..Default::default()
    })
    .execute(json!({"operation":"search","query":"q"}))
    .await
    .unwrap_err();
    let message = error.to_string();
    for guidance in [message.as_str(), WebTool::new().description()] {
        assert!(guidance.contains("Do not automatically bypass search providers"));
        assert!(
            guidance
                .contains("User-requested alternative search or network diagnostics are allowed")
        );
        assert!(guidance.contains("subject to tool permissions"));
        assert!(!guidance.contains("do not use shell"));
        assert!(!guidance.contains("Do not fall back to shell/Python urllib search"));
    }
}

struct Recovering(AtomicUsize);
#[async_trait]
impl SearchProvider for Recovering {
    async fn search(&self, _: &str, _: usize) -> Result<Vec<SearchResult>, ToolError> {
        if self.0.fetch_add(1, Ordering::SeqCst) < 2 {
            Err(ToolError::Execution("503".into()))
        } else {
            Ok(vec![SearchResult::new(
                "recovered",
                "https://example.test/recovered",
                "",
                "recovered",
            )])
        }
    }
}
#[tokio::test]
async fn successful_cooldown_retry_closes_circuit() {
    let router = router(vec![SearchCandidate {
        name: "recovering".into(),
        provider: Arc::new(Recovering(AtomicUsize::new(0))),
        timeout: Duration::from_secs(1),
        fallback: false,
    }]);
    for _ in 0..2 {
        assert!(router.search("q", 1).await.is_err());
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(router.search("q", 1).await.unwrap().len(), 1);
    let stats = router.stats();
    assert_eq!(stats[0].1.consecutive_failures, 0);
    assert_eq!(stats[0].1.successes, 1);
    assert!(stats[0].1.open_until.is_none());
}
#[tokio::test]
async fn builtin_circuit_state_persists_across_calls_and_clones() {
    let primary = server(503, "").await;
    let fallback = server(200, DDG).await;
    let tool = web(SearchConfig {
        brave_api_key: Some("key".into()),
        brave_url: primary.url.clone(),
        duckduckgo_url: fallback.url.clone(),
        circuit_failure_threshold: 2,
        circuit_cooldown: Duration::from_secs(60),
        ..Default::default()
    });
    // A short fallback causes both candidates to finish, so state cannot hide
    // behind sufficient-result cancellation.
    for _ in 0..3 {
        tool.clone()
            .execute(json!({"operation":"search","query":"q","limit":2}))
            .await
            .unwrap();
    }
    assert_eq!(primary.hits.load(Ordering::SeqCst), 2);
    assert_eq!(fallback.hits.load(Ordering::SeqCst), 3);
}
struct FailingDns;
impl reqwest::dns::Resolve for FailingDns {
    fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async { Err(std::io::Error::other("DNS lookup failed").into()) })
    }
}
#[tokio::test]
async fn real_dns_and_connection_failure_fall_back() {
    let fallback = server(200, DDG).await;
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let refused = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    for url in ["http://fixture.invalid".to_owned(), refused] {
        let client = reqwest::Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(FailingDns))
            .build()
            .unwrap();
        let tool = WebTool::new()
            .with_client(client)
            .with_search_config(SearchConfig {
                brave_api_key: Some("key".into()),
                brave_url: url,
                duckduckgo_url: fallback.url.clone(),
                ..Default::default()
            });
        let value: Value = serde_json::from_str(
            &tool
                .execute(json!({"operation":"search","query":"q","limit":1}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["results"][0]["source"], "duckduckgo");
    }
}
#[tokio::test]
async fn adapters_send_correct_methods_credentials_and_query_body() {
    for kind in ["bocha", "brave", "searxng"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                let request = String::from_utf8_lossy(&bytes);
                if let Some((headers, body)) = request.split_once("\r\n\r\n") {
                    let size = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|v| v.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= size {
                        break;
                    }
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            let body = match kind {
                "bocha" => {
                    assert!(request.starts_with("POST /v1/web-search "));
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("authorization: bearer fixture-key")
                    );
                    let payload: Value =
                        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(payload["query"], "ax 中文");
                    assert_eq!(payload["count"], 1);
                    assert_eq!(payload["summary"], false);
                    r#"{"code":200,"data":{"webPages":{"value":[{"name":"title","url":"https://example.test/a","snippet":"text"}]}}}"#
                }
                "brave" => {
                    assert!(request.starts_with("GET /search?q="));
                    assert!(request.contains("count=1"));
                    assert!(
                        request
                            .to_ascii_lowercase()
                            .contains("x-subscription-token: fixture-key")
                    );
                    r#"{"web":{"results":[{"title":"title","url":"https://example.test/a","description":"text"}]}}"#
                }
                _ => {
                    assert!(request.starts_with("GET /base/search?q="));
                    assert!(request.contains("format=json"));
                    r#"{"results":[{"title":"title","url":"https://example.test/a","content":"text"}]}"#
                }
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        let mut config = SearchConfig::default();
        match kind {
            "bocha" => {
                config.bocha_api_key = Some("fixture-key".into());
                config.bocha_url = format!("{url}/v1/web-search");
            }
            "brave" => {
                config.brave_api_key = Some("fixture-key".into());
                config.brave_url = format!("{url}/search");
            }
            _ => config.searxng_url = Some(format!("{url}/base/")),
        }
        web(config)
            .execute(json!({"operation":"search","query":"ax 中文","limit":1}))
            .await
            .unwrap();
        task.await.unwrap();
    }
}
#[tokio::test]
async fn genuine_empty_search_is_success_but_challenge_is_failure() {
    let empty = server(200, "<div class='no-results'>No results</div>").await;
    let tool = web(SearchConfig {
        duckduckgo_url: empty.url.clone(),
        ..Default::default()
    });
    let value: Value = serde_json::from_str(
        &tool
            .execute(json!({"operation":"search","query":"q"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["results"], json!([]));
    assert_eq!(value["failed"], 0);
}
#[tokio::test]
async fn duckduckgo_dom_decodes_entities_redirects_and_excludes_ads() {
    let html = r#"<div class="result result--ad"><a class="result__a" href="https://ad.test">Ad</a></div>
        <div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.test%2Fdoc%3Fa%3D1%26b%3D2&amp;rut=abc">Title &amp; <b>More</b></a><a class="result__snippet">Body &#x4e2d; &amp; info</a></div>
        <div class="result"><a class="result__a" href="javascript:alert(1)">bad</a></div>"#;
    let ddg = server(200, html).await;
    let value: Value = serde_json::from_str(
        &web(SearchConfig {
            duckduckgo_url: ddg.url.clone(),
            ..Default::default()
        })
        .execute(json!({"operation":"search","query":"q","limit":1}))
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
    assert_eq!(value["results"][0]["title"], "Title & More");
    assert_eq!(value["results"][0]["snippet"], "Body 中 & info");
    assert_eq!(
        value["results"][0]["url"],
        "https://example.test/doc?a=1&b=2"
    );
}

#[tokio::test]
async fn configured_provider_total_timeout_is_independent() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_url = format!("http://{}", listener.local_addr().unwrap());
    let slow = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 2048];
        assert!(socket.read(&mut buffer).await.unwrap() > 0);
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let ddg = server(200, DDG).await;
    let mut config = SearchConfig {
        brave_api_key: Some("key".into()),
        brave_url: slow_url,
        duckduckgo_url: ddg.url.clone(),
        hedge_delay: Duration::from_secs(1),
        ..Default::default()
    };
    config.brave_timeout.total = Duration::from_millis(20);
    config.duckduckgo_timeout.total = Duration::from_secs(2);
    let value: Value = serde_json::from_str(
        &tokio::time::timeout(
            Duration::from_millis(500),
            web(config).execute(json!({"operation":"search","query":"q","limit":1})),
        )
        .await
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    assert_eq!(value["results"][0]["source"], "duckduckgo");
    slow.abort();
}
#[tokio::test]
async fn caller_cancellation_drops_router_provider_futures() {
    let (a, _, completed) = candidate("a", 300, None, vec!["https://example.test/a"]);
    let router = router(vec![a]);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), router.search("q", 1))
            .await
            .is_err()
    );
    assert_eq!(router.stats()[0].1.cancellations, 1);
    assert_eq!(router.stats()[0].1.failures, 0);
    tokio::time::sleep(Duration::from_millis(320)).await;
    assert_eq!(completed.load(Ordering::SeqCst), 0);
}
