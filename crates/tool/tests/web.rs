use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tool::{SearchProvider, SearchResult, Tool, WebTool};

struct MockSearch;
#[async_trait]
impl SearchProvider for MockSearch {
    async fn search(
        &self,
        query: &str,
        _limit: usize,
    ) -> Result<Vec<SearchResult>, tool::ToolError> {
        Ok(vec![SearchResult {
            title: query.into(),
            url: "https://example.test".into(),
            snippet: "mock".into(),
            source: "test".into(),
        }])
    }
}

#[tokio::test]
async fn web_uses_provider_and_extracts_html_from_local_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 1024];
        let amount = stream.read(&mut buffer).await.unwrap();
        assert!(amount > 0);
        let body = "<html><head><script>SECRET_SCRIPT</script></head><body><h1>Hello</h1><p>World</p></body></html>";
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    let web = WebTool::new()
        .with_client(reqwest::Client::builder().no_proxy().build().unwrap())
        .with_search_provider(Arc::new(MockSearch));
    let fetched: Value = serde_json::from_str(
        &web.execute(json!({"operation":"fetch","url":format!("http://{address}/")}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(fetched["content"].as_str().unwrap().contains("Hello"));
    assert!(!fetched["content"].as_str().unwrap().contains("<html>"));
    assert!(
        !fetched["content"]
            .as_str()
            .unwrap()
            .contains("SECRET_SCRIPT")
    );
    let searched: Value = serde_json::from_str(
        &web.execute(json!({"operation":"search","query":"ax"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(searched["results"][0]["title"], "ax");
    assert!(
        web.execute(json!({"operation":"fetch","url":"file:///etc/passwd"}))
            .await
            .is_err()
    );
}
