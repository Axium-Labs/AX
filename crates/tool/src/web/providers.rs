//! Independent search adapters; fallback and hedging belong to `SearchRouter`.
use super::{SearchProvider, SearchResult, ToolError, network};
use async_trait::async_trait;
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
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
    pub searxng_url: Option<String>,
    pub duckduckgo_url: String,
    pub bocha_timeout: ProviderTimeout,
    pub brave_timeout: ProviderTimeout,
    pub searxng_timeout: ProviderTimeout,
    pub duckduckgo_timeout: ProviderTimeout,
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
            searxng_url: None,
            duckduckgo_url: "https://html.duckduckgo.com/html/".into(),
            bocha_timeout: ProviderTimeout::default(),
            brave_timeout: ProviderTimeout::default(),
            searxng_timeout: ProviderTimeout::default(),
            duckduckgo_timeout: ProviderTimeout::default(),
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
            searxng_url: env("AX_SEARCH_SEARXNG_URL"),
            bocha_timeout: ProviderTimeout::from_env("BOCHA"),
            brave_timeout: ProviderTimeout::from_env("BRAVE"),
            searxng_timeout: ProviderTimeout::from_env("SEARXNG"),
            duckduckgo_timeout: ProviderTimeout::from_env("DUCKDUCKGO"),
            ..Self::default()
        }
    }
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
pub struct SearxngSearch {
    pub client: reqwest::Client,
    pub url: String,
}
pub struct DuckDuckGoSearch {
    pub client: reqwest::Client,
    pub url: String,
}
async fn json_body(request: reqwest::RequestBuilder, url: &str) -> Result<Value, ToolError> {
    let body = network::search_request(request, url, false)
        .await
        .map_err(|e| ToolError::Execution(e.error))?;
    serde_json::from_slice(&body.bytes).map_err(|_| {
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
#[async_trait]
impl SearchProvider for SearxngSearch {
    async fn search(&self, query: &str, _: usize) -> Result<Vec<SearchResult>, ToolError> {
        let mut url =
            reqwest::Url::parse(&self.url).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if !url.path().trim_end_matches('/').ends_with("/search") {
            url.set_path(&format!("{}/search", url.path().trim_end_matches('/')));
        }
        let value = json_body(
            self.client
                .get(url.clone())
                .query(&[("q", query), ("format", "json")]),
            url.as_str(),
        )
        .await?;
        results(&value, "/results", "title", "content", "searxng")
    }
}
#[async_trait]
impl SearchProvider for DuckDuckGoSearch {
    async fn search(&self, query: &str, _limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let body = network::search_request(
            self.client.get(&self.url).query(&[("q", query)]),
            &self.url,
            true,
        )
        .await
        .map_err(|e| ToolError::Execution(e.error))?;
        let html =
            String::from_utf8(body.bytes).map_err(|e| ToolError::Execution(e.to_string()))?;
        parse_duckduckgo(&html, usize::MAX)
    }
}
fn has_class(node: &Handle, class: &str) -> bool {
    let NodeData::Element { attrs, .. } = &node.data else {
        return false;
    };
    attrs.borrow().iter().any(|attribute| {
        attribute.name.local.as_ref() == "class"
            && attribute
                .value
                .split_ascii_whitespace()
                .any(|value| value == class)
    })
}

fn find_class(node: &Handle, class: &str) -> Option<Handle> {
    if has_class(node, class) {
        return Some(node.clone());
    }
    node.children
        .borrow()
        .iter()
        .find_map(|child| find_class(child, class))
}

fn text(node: &Handle) -> String {
    fn append(node: &Handle, output: &mut String) {
        if let NodeData::Text { contents } = &node.data {
            output.push_str(&contents.borrow());
        }
        for child in node.children.borrow().iter() {
            append(child, output);
        }
    }
    let mut output = String::new();
    append(node, &mut output);
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn valid_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    matches!(url.scheme(), "http" | "https").then(|| url.to_string())
}

fn result_url(link: &Handle) -> Option<String> {
    let NodeData::Element { attrs, .. } = &link.data else {
        return None;
    };
    let attributes = attrs.borrow();
    let raw = &attributes
        .iter()
        .find(|attribute| attribute.name.local.as_ref() == "href")?
        .value;
    let base = reqwest::Url::parse("https://html.duckduckgo.com/").ok()?;
    let url = base.join(raw).ok()?;
    if url
        .host_str()
        .is_some_and(|host| host == "duckduckgo.com" || host.ends_with(".duckduckgo.com"))
    {
        let target = url.query_pairs().find(|(key, _)| key == "uddg")?.1;
        return valid_url(&target);
    }
    valid_url(url.as_str())
}

fn parse_duckduckgo(html: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
    fn visit(node: &Handle, limit: usize, results: &mut Vec<SearchResult>) {
        if results.len() >= limit {
            return;
        }
        if has_class(node, "result") {
            if has_class(node, "result--ad") {
                return;
            }
            if let Some(link) = find_class(node, "result__a")
                && let Some(url) = result_url(&link)
            {
                let title = text(&link);
                if !title.is_empty() {
                    let snippet = find_class(node, "result__snippet")
                        .map_or_else(String::new, |node| text(&node));
                    results.push(SearchResult::new(title, url, snippet, "duckduckgo"));
                }
            }
            return;
        }
        for child in node.children.borrow().iter() {
            visit(child, limit, results);
        }
    }
    let dom = parse_document(RcDom::default(), html5ever::ParseOpts::default()).one(html);
    let mut results = Vec::new();
    visit(&dom.document, limit, &mut results);
    if results.is_empty() && find_class(&dom.document, "no-results").is_none() {
        return Err(ToolError::Execution(
            "DuckDuckGo HTML search returned a challenge or unrecognized page".into(),
        ));
    }
    Ok(results)
}
