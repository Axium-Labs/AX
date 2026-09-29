use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Barrier,
};
use tool::{
    MAX_FETCH_URLS, MAX_PAGE_CHARS, MAX_QUERIES, MAX_TOTAL_CHARS, SearchProvider, SearchResult,
    Tool, ToolError, WebTool,
};

struct MockSearch {
    barrier: Option<Arc<Barrier>>,
    fail: Vec<String>,
    results: fn(&str) -> Vec<SearchResult>,
}

impl Default for MockSearch {
    fn default() -> Self {
        Self {
            barrier: None,
            fail: Vec::new(),
            results: one_result,
        }
    }
}

fn one_result(query: &str) -> Vec<SearchResult> {
    vec![SearchResult::new(
        query,
        format!("https://example.test/{query}"),
        "mock snippet",
        "test",
    )]
}

#[async_trait]
impl SearchProvider for MockSearch {
    async fn search(&self, query: &str, _limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        if self.fail.iter().any(|failed| failed == query) {
            return Err(ToolError::Execution(format!("search failed for {query}")));
        }
        Ok((self.results)(query))
    }
}

fn tool_with_search(search: MockSearch) -> WebTool {
    WebTool::new()
        .with_client(reqwest::Client::builder().no_proxy().build().unwrap())
        .with_search_provider(Arc::new(search))
}

fn tool_without_search() -> WebTool {
    WebTool::new().with_client(reqwest::Client::builder().no_proxy().build().unwrap())
}

fn payload(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn search_payload(text: &str) -> Value {
    let value = payload(text);
    assert!(value["results"].is_array(), "{value}");
    value
}

struct TestServer {
    address: SocketAddr,
    peak_in_flight: Arc<AtomicUsize>,
}

/// Serves one request per connection after an optional delay, tracking how many
/// requests are in flight at the same time.
async fn spawn_server<F>(handler: F, delay: Duration) -> TestServer
where
    F: Fn(&str) -> (u16, &'static str, String) + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak_in_flight = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&in_flight);
    let peak = Arc::clone(&peak_in_flight);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let handler = Arc::clone(&handler);
            let counter = Arc::clone(&counter);
            let peak = Arc::clone(&peak);
            tokio::spawn(async move {
                let current = counter.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                let mut buffer = [0u8; 2048];
                let Ok(read) = stream.read(&mut buffer).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let (code, reason, body) = handler(&path);
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                let response = format!(
                    "HTTP/1.1 {code} {reason}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                counter.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    TestServer {
        address,
        peak_in_flight,
    }
}

#[tokio::test]
async fn search_accepts_multiple_queries_concurrently() {
    let barrier = Arc::new(Barrier::new(3));
    let tool = tool_with_search(MockSearch {
        barrier: Some(barrier),
        ..Default::default()
    });
    let executed = tokio::time::timeout(
        Duration::from_secs(5),
        tool.execute(json!({"operation":"search","queries":["alpha","beta","gamma"],"limit":5})),
    )
    .await
    .expect("queries must be searched concurrently");
    let value = search_payload(&executed.unwrap());
    assert_eq!(value["queries"].as_array().unwrap().len(), 3);
    assert_eq!(value["succeeded"], 3);
    assert_eq!(value["failed"], 0);
    assert_eq!(value["results"].as_array().unwrap().len(), 3);
    assert_eq!(value["results"][0]["matched_queries"][0], "alpha");
    assert_eq!(value["results"][2]["title"], "gamma");
}

#[tokio::test]
async fn search_keeps_legacy_single_query() {
    let tool = tool_with_search(MockSearch::default());
    let value = search_payload(
        &tool
            .execute(json!({"operation":"search","query":"ax"}))
            .await
            .unwrap(),
    );
    assert_eq!(value["queries"][0], "ax");
    assert_eq!(value["results"][0]["title"], "ax");
    assert_eq!(value["results"][0]["url"], "https://example.test/ax");
    assert_eq!(value["results"][0]["matched_queries"][0], "ax");
    assert!(value["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn search_merges_and_deduplicates_urls() {
    fn overlapping(query: &str) -> Vec<SearchResult> {
        match query {
            "first" => vec![
                SearchResult::new(
                    "Page",
                    "https://example.test/page?utm_source=news&keep=1#section",
                    "one",
                    "test",
                ),
                SearchResult::new("Shared", "https://example.test/shared/", "two", "test"),
            ],
            "second" => vec![
                SearchResult::new(
                    "Shared dup",
                    "https://example.test/shared?fbclid=abc",
                    "three",
                    "test",
                ),
                SearchResult::new(
                    "Page dup",
                    "https://example.test/page?keep=1",
                    "four",
                    "test",
                ),
            ],
            _ => Vec::new(),
        }
    }
    let tool = tool_with_search(MockSearch {
        results: overlapping,
        ..Default::default()
    });
    let value = search_payload(
        &tool
            .execute(json!({"operation":"search","queries":["first","second"]}))
            .await
            .unwrap(),
    );
    let results = value["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "duplicate URLs must be merged: {value}");
    assert_eq!(
        results[0]["url"],
        "https://example.test/page?utm_source=news&keep=1#section"
    );
    assert_eq!(results[0]["title"], "Page");
    assert_eq!(results[0]["matched_queries"], json!(["first", "second"]));
    assert_eq!(results[1]["matched_queries"], json!(["first", "second"]));
}

#[tokio::test]
async fn search_reports_partial_failures() {
    let tool = tool_with_search(MockSearch {
        fail: vec!["bad".into()],
        ..Default::default()
    });
    let value = search_payload(
        &tool
            .execute(json!({"operation":"search","queries":["good","bad","other"]}))
            .await
            .unwrap(),
    );
    assert_eq!(value["succeeded"], 2);
    assert_eq!(value["failed"], 1);
    assert_eq!(value["results"].as_array().unwrap().len(), 2);
    assert_eq!(value["errors"][0]["query"], "bad");
    assert!(
        value["errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("search failed for bad")
    );
}

#[tokio::test]
async fn search_fails_only_when_every_query_fails() {
    let tool = tool_with_search(MockSearch {
        fail: vec!["alpha".into(), "beta".into()],
        ..Default::default()
    });
    let error = tool
        .execute(json!({"operation":"search","queries":["alpha","beta"]}))
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("every query"), "{message}");
    assert!(message.contains("alpha:"), "{message}");
    assert!(message.contains("beta:"), "{message}");
    assert!(message.contains("search failed for alpha"), "{message}");
}

#[tokio::test]
async fn search_validates_query_list() {
    let tool = tool_with_search(MockSearch::default());
    let too_many = tool
        .execute(json!({"operation":"search","queries":["a","b","c","d","e"]}))
        .await
        .unwrap_err();
    assert!(too_many.to_string().contains("at most 4"), "{too_many}");
    let empty = tool
        .execute(json!({"operation":"search","queries":[]}))
        .await
        .unwrap_err();
    assert!(empty.to_string().contains("1-4"), "{empty}");
    let blank = tool
        .execute(json!({"operation":"search","queries":["  "]}))
        .await
        .unwrap_err();
    assert!(blank.to_string().contains("empty"), "{blank}");
    let limit = tool
        .execute(json!({"operation":"search","query":"a","limit":0}))
        .await
        .unwrap_err();
    assert!(limit.to_string().contains("limit"), "{limit}");
}

#[tokio::test]
async fn search_deduplicates_repeated_queries_within_limit() {
    let tool = tool_with_search(MockSearch::default());
    let value = search_payload(
        &tool
            .execute(json!({"operation":"search","queries":["ax","ax","ax","ax","ax"]}))
            .await
            .unwrap(),
    );
    assert_eq!(value["queries"].as_array().unwrap().len(), 1);
    assert_eq!(value["results"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn search_reports_missing_provider() {
    let tool = tool_without_search();
    let error = tool
        .execute(json!({"operation":"search","queries":["ax"]}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("BRAVE_SEARCH_API_KEY"));
    assert_eq!(MAX_QUERIES, 4);
}

const PAGE: &str = "<html><head><title>Doc Title</title>\
<style>SECRET_STYLE</style><script>SECRET_SCRIPT</script></head>\
<body><nav>NAV_NOISE</nav><h1>Hello</h1><p>World</p>\
<footer>FOOTER_NOISE</footer></body></html>";

#[tokio::test]
async fn fetch_keeps_legacy_single_url_and_cleans_text() {
    let server = spawn_server(|_| (200, "OK", PAGE.to_owned()), Duration::ZERO).await;
    let tool = tool_without_search();
    let value = payload(
        &tool
            .execute(json!({"operation":"fetch","url":format!("http://{}/doc", server.address)}))
            .await
            .unwrap(),
    );
    let pages = value["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(value["succeeded"], 1);
    assert_eq!(pages[0]["title"], "Doc Title");
    assert_eq!(pages[0]["truncated"], false);
    assert!(pages[0]["url"].as_str().unwrap().ends_with("/doc"));
    let content = pages[0]["content"].as_str().unwrap();
    assert!(content.contains("Hello"), "{content}");
    assert!(content.contains("World"), "{content}");
    for noise in [
        "SECRET_SCRIPT",
        "SECRET_STYLE",
        "NAV_NOISE",
        "FOOTER_NOISE",
        "<html>",
    ] {
        assert!(!content.contains(noise), "{noise} leaked into {content}");
    }
}

#[tokio::test]
async fn fetch_runs_urls_concurrently() {
    let server = spawn_server(
        |_| {
            (
                200,
                "OK",
                "<html><body><p>body</p></body></html>".to_owned(),
            )
        },
        Duration::from_millis(150),
    )
    .await;
    let tool = tool_without_search();
    let urls: Vec<String> = ["/a", "/b", "/c"]
        .iter()
        .map(|path| format!("http://{}{path}", server.address))
        .collect();
    let value = payload(
        &tool
            .execute(json!({"operation":"fetch","urls":urls}))
            .await
            .unwrap(),
    );
    assert_eq!(value["pages"].as_array().unwrap().len(), 3);
    assert_eq!(value["succeeded"], 3);
    assert!(
        server.peak_in_flight.load(Ordering::SeqCst) >= 2,
        "urls must be fetched concurrently"
    );
}

#[tokio::test]
async fn fetch_reports_partial_failures() {
    let server = spawn_server(
        |path| {
            if path.starts_with("/ok") {
                (200, "OK", "<html><body>fine</body></html>".to_owned())
            } else {
                (404, "Not Found", "missing".to_owned())
            }
        },
        Duration::ZERO,
    )
    .await;
    let tool = tool_without_search();
    let value = payload(
        &tool
            .execute(json!({
                "operation":"fetch",
                "urls":[format!("http://{}/ok", server.address), format!("http://{}/missing", server.address)]
            }))
            .await
            .unwrap(),
    );
    assert_eq!(value["succeeded"], 1);
    assert_eq!(value["failed"], 1);
    assert_eq!(value["pages"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["errors"][0]["url"],
        format!("http://{}/missing", server.address)
    );
    assert!(
        value["errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("404")
    );
}

#[tokio::test]
async fn fetch_fails_only_when_every_url_fails() {
    let server = spawn_server(|_| (500, "Server Error", String::new()), Duration::ZERO).await;
    let tool = tool_without_search();
    let error = tool
        .execute(json!({
            "operation":"fetch",
            "urls":[format!("http://{}/a", server.address), format!("http://{}/b", server.address)]
        }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("every url"), "{error}");
    let scheme = tool
        .execute(json!({"operation":"fetch","url":"file:///etc/passwd"}))
        .await
        .unwrap_err();
    assert!(scheme.to_string().contains("HTTP(S)"), "{scheme}");
}

#[tokio::test]
async fn fetch_validates_url_list() {
    let tool = tool_without_search();
    let urls: Vec<String> = (0..=MAX_FETCH_URLS)
        .map(|index| format!("http://127.0.0.1:1/{index}"))
        .collect();
    let tool_many = tool
        .execute(json!({"operation":"fetch","urls":urls}))
        .await
        .unwrap_err();
    assert!(tool_many.to_string().contains("at most 6"), "{tool_many}");
    let empty = tool
        .execute(json!({"operation":"fetch","urls":[]}))
        .await
        .unwrap_err();
    assert!(empty.to_string().contains("1-6"), "{empty}");
}

#[tokio::test]
async fn fetch_truncates_large_pages() {
    let body = format!("<html><body><p>{}</p></body></html>", "x".repeat(50_000));
    let server = spawn_server(move |_| (200, "OK", body.clone()), Duration::ZERO).await;
    let tool = tool_without_search();
    let value = payload(
        &tool
            .execute(json!({"operation":"fetch","url":format!("http://{}/big", server.address)}))
            .await
            .unwrap(),
    );
    let page = &value["pages"][0];
    assert_eq!(page["truncated"], true);
    let content = page["content"].as_str().unwrap();
    assert_eq!(content.chars().count(), MAX_PAGE_CHARS);
    assert!(content.chars().all(|c| c == 'x'), "{content}");
}

#[tokio::test]
async fn fetch_applies_a_total_output_budget() {
    let body = format!(
        "<html><body><p>{}</p></body></html>",
        "y".repeat(MAX_PAGE_CHARS)
    );
    let server = spawn_server(move |_| (200, "OK", body.clone()), Duration::ZERO).await;
    let tool = tool_without_search();
    let urls: Vec<String> = (0..MAX_FETCH_URLS)
        .map(|index| format!("http://{}/page{index}", server.address))
        .collect();
    let value = payload(
        &tool
            .execute(json!({"operation":"fetch","urls":urls}))
            .await
            .unwrap(),
    );
    let pages = value["pages"].as_array().unwrap();
    assert_eq!(pages.len(), MAX_FETCH_URLS);
    let total: usize = pages
        .iter()
        .map(|page| page["content"].as_str().unwrap().chars().count())
        .sum();
    assert!(total <= MAX_TOTAL_CHARS, "{total} exceeds the budget");
    assert_eq!(pages[0]["truncated"], false);
    assert_eq!(pages[MAX_FETCH_URLS - 1]["truncated"], true);
    assert_eq!(pages[MAX_FETCH_URLS - 1]["content"], "");
}
