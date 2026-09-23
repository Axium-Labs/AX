//! Side-effect-free web search and bounded page retrieval.
use crate::{Capability, SafetyLevel, Tool, ToolError};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

const MAX_BYTES: usize = 2_000_000;
const MAX_TEXT: usize = 100_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source: String,
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError>;
}

struct BraveSearch {
    client: reqwest::Client,
    api_key: String,
}

#[async_trait]
impl SearchProvider for BraveSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let response = self
            .client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", &self.api_key)
            .query(&[("q", query), ("count", &limit.to_string())])
            .send()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?
            .error_for_status()
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let value: Value = response
            .json()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        Ok(value
            .pointer("/web/results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(limit)
            .map(|entry| SearchResult {
                title: entry["title"].as_str().unwrap_or_default().to_owned(),
                url: entry["url"].as_str().unwrap_or_default().to_owned(),
                snippet: entry["description"].as_str().unwrap_or_default().to_owned(),
                source: "brave".to_owned(),
            })
            .collect())
    }
}

pub struct WebTool {
    client: reqwest::Client,
    search: Option<Arc<dyn SearchProvider>>,
}

impl Default for WebTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebTool {
    #[must_use]
    /// Builds the bounded HTTP client; its static configuration is valid on supported platforms.
    ///
    /// # Panics
    ///
    /// Panics only if the HTTP client builder rejects its static configuration.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("valid web client");
        let search = std::env::var("BRAVE_SEARCH_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|api_key| {
                Arc::new(BraveSearch {
                    client: client.clone(),
                    api_key,
                }) as Arc<dyn SearchProvider>
            });
        Self { client, search }
    }

    #[must_use]
    pub fn with_search_provider(mut self, provider: Arc<dyn SearchProvider>) -> Self {
        self.search = Some(provider);
        self
    }

    #[must_use]
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    async fn fetch(&self, url: &str) -> Result<String, ToolError> {
        let parsed =
            reqwest::Url::parse(url).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ToolError::InvalidInput(
                "web fetch only accepts HTTP(S) URLs".into(),
            ));
        }
        let response = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?
            .error_for_status()
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !(content_type.starts_with("text/html")
            || content_type.starts_with("text/plain")
            || content_type.starts_with("text/markdown"))
        {
            return Err(ToolError::Execution(format!(
                "unsupported content type: {content_type}"
            )));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(ToolError::Execution(
                "web response exceeds size limit".into(),
            ));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ToolError::Execution(e.to_string()))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(ToolError::Execution(
                    "web response exceeds size limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8(bytes).map_err(|e| ToolError::Execution(e.to_string()))?;
        let clean = if content_type.starts_with("text/html") {
            html2md::parse_html(&text)
        } else {
            text
        };
        let content: String = clean.chars().take(MAX_TEXT).collect();
        Ok(json!({"url":final_url,"content_type":content_type,"content":content,"truncated":clean.chars().count()>MAX_TEXT}).to_string())
    }
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Search {
        query: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Fetch {
        url: String,
    },
}
const fn default_limit() -> usize {
    5
}

#[async_trait]
impl Tool for WebTool {
    fn name(&self) -> &'static str {
        "web"
    }
    fn description(&self) -> &'static str {
        "Search the web or fetch an HTTP(S) page as clean Markdown/text. Read-only GET requests."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"operation":{"type":"string","enum":["search","fetch"]},"query":{"type":"string"},"url":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":20}},"required":["operation"],"additionalProperties":false})
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::Network
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        match input {
            Input::Fetch { url } => self.fetch(&url).await,
            Input::Search { query, limit } => {
                if query.trim().is_empty() || !(1..=20).contains(&limit) {
                    return Err(ToolError::InvalidInput(
                        "query must be nonempty and limit 1..20".into(),
                    ));
                }
                let provider = self.search.as_ref().ok_or_else(|| ToolError::Execution(
                    "web search unavailable: set BRAVE_SEARCH_API_KEY or configure a SearchProvider".into()))?;
                Ok(json!({"results":provider.search(&query, limit).await?}).to_string())
            }
        }
    }
}
