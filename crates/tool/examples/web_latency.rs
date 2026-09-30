//! Reproducible scheduler benchmark, plus an opt-in public-provider smoke test.
use async_trait::async_trait;
use futures_util::future::join_all;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tool::{SearchProvider, SearchResult, Tool, ToolError, WebTool};

struct SearchFixture;
#[async_trait]
impl SearchProvider for SearchFixture {
    async fn search(&self, query: &str, _: usize) -> Result<Vec<SearchResult>, ToolError> {
        tokio::time::sleep(if query == "fast" {
            Duration::from_millis(20)
        } else {
            Duration::from_millis(300)
        })
        .await;
        Ok(vec![SearchResult::new(
            query,
            format!("https://example.test/{query}"),
            "snippet",
            "fixture",
        )])
    }
}

fn percentile(values: &mut [Duration], percent: usize) -> f64 {
    values.sort_unstable();
    values[(values.len() * percent).div_ceil(100).saturating_sub(1)].as_secs_f64() * 1000.0
}

async fn live() -> Result<(), Box<dyn std::error::Error>> {
    let result = WebTool::new()
        .with_search_config(tool::SearchConfig::default())
        .execute(
            json!({"operation":"search","query":"Rust programming language official","limit":3}),
        )
        .await?;
    println!("{result}");
    for (name, metric) in tool::telemetry::snapshot() {
        if name.starts_with("web.http.") {
            println!(
                "{name}: count={} total_us={} max_us={}",
                metric.count, metric.total_micros, metric.max_micros
            );
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|argument| argument == "--live") {
        return live().await;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut bytes = [0; 2048];
                let Ok(read) = socket.read(&mut bytes).await else {
                    return;
                };
                if read == 0 {
                    return;
                }
                let fast = String::from_utf8_lossy(&bytes[..read]).contains("/fast");
                tokio::time::sleep(if fast {
                    Duration::from_millis(20)
                } else {
                    Duration::from_millis(300)
                })
                .await;
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody").await;
            });
        }
    });
    let client = reqwest::Client::builder().no_proxy().build()?;
    let tool = WebTool::new()
        .with_client(client.clone())
        .with_search_provider(Arc::new(SearchFixture));
    let urls = [
        format!("http://{address}/slow"),
        format!("http://{address}/fast"),
        format!("http://{address}/other"),
    ];
    let queries = ["slow", "fast", "other"];
    let mut search_before = Vec::new();
    let mut search_after = Vec::new();
    let mut fetch_before = Vec::new();
    let mut fetch_after = Vec::new();
    // Warm up both paths, then use the same client, fixtures and valid-result
    // target. The baseline is AX's previous join_all/wait-for-all scheduler.
    for round in 0..31 {
        let started = Instant::now();
        let baseline = join_all(queries.iter().map(|query| SearchFixture.search(query, 1))).await;
        assert!(baseline.into_iter().all(|result| result.is_ok()));
        let before_search = started.elapsed();
        let started = Instant::now();
        let result: Value = serde_json::from_str(
            &tool
                .execute(json!({"operation":"search","queries":queries,"limit":1}))
                .await?,
        )?;
        assert_eq!(result["results"].as_array().unwrap().len(), 1);
        let after_search = started.elapsed();
        let started = Instant::now();
        let baseline = join_all(urls.iter().map(|url| {
            let client = &client;
            async move {
                client
                    .get(url)
                    .send()
                    .await?
                    .error_for_status()?
                    .text()
                    .await
            }
        }))
        .await;
        assert!(baseline.into_iter().all(|result| result.is_ok()));
        let before_fetch = started.elapsed();
        let started = Instant::now();
        let result: Value = serde_json::from_str(
            &tool
                .execute(json!({"operation":"fetch","urls":urls,"target_pages":1}))
                .await?,
        )?;
        assert_eq!(result["succeeded"], 1);
        let after_fetch = started.elapsed();
        if round > 0 {
            search_before.push(before_search);
            search_after.push(after_search);
            fetch_before.push(before_fetch);
            fetch_after.push(after_fetch);
        }
    }
    let report = json!({
        "samples":30,"fixture":{"fast_ms":20,"straggler_ms":300,"target":1,"transport":"loopback, shared client"},
        "search":{"before_p50_ms":percentile(&mut search_before,50),"before_p95_ms":percentile(&mut search_before,95),"after_p50_ms":percentile(&mut search_after,50),"after_p95_ms":percentile(&mut search_after,95)},
        "fetch":{"before_p50_ms":percentile(&mut fetch_before,50),"before_p95_ms":percentile(&mut fetch_before,95),"after_p50_ms":percentile(&mut fetch_after,50),"after_p95_ms":percentile(&mut fetch_after,95)}
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    server.abort();
    Ok(())
}
