//! Independent search adapters; fallback and hedging belong to `SearchRouter`.
use super::{SearchProvider, SearchResult, ToolError, network};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
#[derive(Clone, Debug)]
pub struct ProviderTimeout {
    pub connect: Duration,
    pub read: Duration,
    pub total: Duration,
}
impl Default for ProviderTimeout {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(5),
            read: Duration::from_secs(10),
            total: Duration::from_secs(15),
        }
    }
}
impl ProviderTimeout {
    fn from_env(name: &str) -> Self {
        let mut value = Self::default();
        for (suffix, target) in [
            ("CONNECT_TIMEOUT_MS", &mut value.connect),
            ("READ_TIMEOUT_MS", &mut value.read),
            ("TIMEOUT_MS", &mut value.total),
        ] {
            if let Some(ms) = std::env::var(format!("AX_SEARCH_{name}_{suffix}"))
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|ms| *ms > 0)
            {
                *target = Duration::from_millis(ms);
            }
        }
        value
    }
    pub(super) fn client(&self) -> reqwest::Client {
        network::search_client_builder()
            .connect_timeout(self.connect)
            .read_timeout(self.read)
            .timeout(self.total)
            .build()
            .expect("valid search client")
    }
}
#[derive(Clone, Debug)]
pub struct SearchConfig {
    pub bocha_api_key: Option<String>,
    pub bocha_url: String,
    pub brave_api_key: Option<String>,
    pub brave_url: String,
    pub exa_url: Option<String>,
    pub parallel_url: Option<String>,
    pub bocha_timeout: ProviderTimeout,
    pub brave_timeout: ProviderTimeout,
    pub exa_timeout: ProviderTimeout,
    pub parallel_timeout: ProviderTimeout,
    pub native_timeout: Duration,
    pub task_timeout: Duration,
    pub hedge_delay: Duration,
    pub circuit_failure_threshold: u32,
    pub circuit_cooldown: Duration,
}
impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            bocha_api_key: None,
            bocha_url: "https://api.bocha.cn/v1/web-search".into(),
            brave_api_key: None,
            brave_url: "https://api.search.brave.com/res/v1/web/search".into(),
            exa_url: Some("https://mcp.exa.ai/mcp".into()),
            parallel_url: Some("https://search.parallel.ai/mcp".into()),
            bocha_timeout: ProviderTimeout::default(),
            brave_timeout: ProviderTimeout::default(),
            exa_timeout: ProviderTimeout::default(),
            parallel_timeout: ProviderTimeout::default(),
            native_timeout: Duration::from_secs(12),
            task_timeout: Duration::from_secs(30),
            hedge_delay: Duration::from_millis(500),
            circuit_failure_threshold: 3,
            circuit_cooldown: Duration::from_secs(30),
        }
    }
}
impl SearchConfig {
    pub(super) fn from_env() -> Self {
        let env = |name| std::env::var(name).ok().filter(|s| !s.trim().is_empty());
        Self {
            bocha_api_key: env("BOCHA_SEARCH_API_KEY"),
            brave_api_key: env("BRAVE_SEARCH_API_KEY"),
            exa_url: env("AX_SEARCH_EXA_URL").or_else(|| Self::default().exa_url),
            parallel_url: env("AX_SEARCH_PARALLEL_URL").or_else(|| Self::default().parallel_url),
            bocha_timeout: ProviderTimeout::from_env("BOCHA"),
            brave_timeout: ProviderTimeout::from_env("BRAVE"),
            exa_timeout: ProviderTimeout::from_env("EXA"),
            parallel_timeout: ProviderTimeout::from_env("PARALLEL"),
            native_timeout: duration_env("AX_SEARCH_NATIVE_TIMEOUT_MS", 12000),
            task_timeout: duration_env("AX_SEARCH_TASK_TIMEOUT_MS", 30000),
            ..Self::default()
        }
    }
}
fn duration_env(name: &str, default: u64) -> Duration {
    Duration::from_millis(
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|ms| *ms > 0)
            .unwrap_or(default),
    )
}
pub struct BochaSearch {
    pub client: reqwest::Client,
    pub url: String,
    pub key: String,
}
pub struct BraveSearch {
    pub client: reqwest::Client,
    pub url: String,
    pub key: String,
}
async fn json_body(request: reqwest::RequestBuilder, url: &str) -> Result<Value, ToolError> {
    let _timer = super::telemetry::Timer::new("web.http.total");
    let response = request.send().await.map_err(|_| {
        ToolError::Execution(format!(
            "search request failed for {}",
            reqwest::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default()
        ))
    })?;
    if !response.status().is_success() {
        return Err(ToolError::SearchHttp {
            status: response.status().as_u16(),
            retry_after: model::retry::retry_after_header(response.headers()),
        });
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| ToolError::Execution("search response read failed".into()))?;
        if bytes.len() + chunk.len() > super::MAX_BYTES {
            return Err(ToolError::Execution(
                "search response exceeds size limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        ToolError::Execution("search returned a challenge or invalid JSON response".into())
    })
}
fn results(
    value: &Value,
    pointer: &str,
    title: &str,
    snippet: &str,
    source: &str,
) -> Result<Vec<SearchResult>, ToolError> {
    let entries = value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ToolError::Execution(format!(
                "{source}: missing result array (API error or challenge)"
            ))
        })?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let url = valid_url(entry["url"].as_str()?)?;
            let title = entry[title].as_str()?.trim();
            if title.is_empty() {
                return None;
            }
            Some(SearchResult::new(
                title,
                url,
                entry[snippet].as_str().unwrap_or_default(),
                source,
            ))
        })
        .collect())
}
#[async_trait]
impl SearchProvider for BochaSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let request = self
            .client
            .post(&self.url)
            .bearer_auth(&self.key)
            .json(&json!({
                "query": query,
                "freshness": "noLimit",
                "summary": false,
                "count": limit.clamp(1, 50),
            }));
        let value = json_body(request, &self.url).await?;
        if value
            .get("code")
            .is_some_and(|code| code.as_u64() != Some(200))
        {
            return Err(ToolError::Execution("bocha: API error".into()));
        }
        results(
            &value,
            if value.get("data").is_some() {
                "/data/webPages/value"
            } else {
                "/webPages/value"
            },
            "name",
            "snippet",
            "bocha",
        )
    }
}
#[async_trait]
impl SearchProvider for BraveSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let value = json_body(
            self.client
                .get(&self.url)
                .header("X-Subscription-Token", &self.key)
                .query(&[("q", query), ("count", &limit.clamp(1, 20).to_string())]),
            &self.url,
        )
        .await?;
        results(&value, "/web/results", "title", "description", "brave")
    }
}
fn valid_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}
