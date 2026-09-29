//! Side-effect-free web search and bounded page retrieval.
//!
//! One tool call may carry several independent queries or URLs. They are
//! executed concurrently, merged, deduplicated and returned as one structured
//! result so the agent loop needs fewer model round trips.
use crate::{Capability, SafetyLevel, Tool, ToolError, telemetry};
use async_trait::async_trait;
use futures_util::{StreamExt, future::join_all};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_BYTES: usize = 2_000_000;
const MAX_LIMIT: usize = 20;
const MAX_TITLE_CHARS: usize = 300;
/// Maximum number of queries accepted by one `search` operation.
pub const MAX_QUERIES: usize = 4;
/// Maximum number of URLs accepted by one `fetch` operation.
pub const MAX_FETCH_URLS: usize = 6;
/// Maximum extracted characters returned for one page.
pub const MAX_PAGE_CHARS: usize = 20_000;
/// Maximum extracted characters returned by one `fetch` operation.
pub const MAX_TOTAL_CHARS: usize = 60_000;
/// Query-parameter prefixes that only identify campaigns or referrers.
const TRACKING_PREFIXES: [&str; 3] = ["utm_", "pk_", "mtm_"];
/// Exact query parameters (lowercased) that never change the page content.
const TRACKING_PARAMS: [&str; 15] = [
    "fbclid",
    "gclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "yclid",
    "mc_cid",
    "mc_eid",
    "igshid",
    "mkt_tok",
    "_ga",
    "_gl",
    "si",
    "spm",
    "oly_anon_id",
];
/// Elements whose text is never page content.
const NOISE_ELEMENTS: [&str; 9] = [
    "script", "style", "noscript", "template", "svg", "form", "nav", "footer", "aside",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source: String,
    /// Queries that surfaced this URL after merging; filled in by `WebTool`.
    #[serde(default)]
    pub matched_queries: Vec<String>,
}

impl SearchResult {
    /// Builds a provider result. `matched_queries` is added by the tool when
    /// results from several queries are merged.
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            source: source.into(),
            matched_queries: Vec::new(),
        }
    }
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
            .map(|entry| {
                SearchResult::new(
                    entry["title"].as_str().unwrap_or_default(),
                    entry["url"].as_str().unwrap_or_default(),
                    entry["description"].as_str().unwrap_or_default(),
                    "brave",
                )
            })
            .collect())
    }
}

/// One page extracted by `fetch`, before the call-wide character budget.
#[derive(Debug, Serialize)]
struct Page {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    content_type: String,
    content: String,
    truncated: bool,
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

    /// Runs every query at the same time and merges the results.
    async fn search_many(&self, queries: &[String], limit: usize) -> Result<String, ToolError> {
        let provider =
            self.search.as_ref().ok_or_else(|| {
                ToolError::Execution(
            "web search unavailable: set BRAVE_SEARCH_API_KEY or configure a SearchProvider".into())
            })?;
        let started = Instant::now();
        let outcomes = join_all(queries.iter().enumerate().map(|(index, query)| {
            let provider = Arc::clone(provider);
            async move {
                let query_started = Instant::now();
                let outcome = provider.search(query, limit).await;
                let elapsed = query_started.elapsed();
                telemetry::record(&format!("web.search.query.{index}"), elapsed);
                telemetry::record("web.search.query", elapsed);
                telemetry::increment(if outcome.is_ok() {
                    "web.search.ok"
                } else {
                    "web.search.failed"
                });
                outcome
            }
        }))
        .await;
        telemetry::record("web.search", started.elapsed());
        let failed = outcomes.iter().filter(|outcome| outcome.is_err()).count();
        if failed == queries.len() {
            return Err(ToolError::Execution(format!(
                "web search failed for every query: {}",
                describe(&outcomes, queries, "query")
            )));
        }
        let results = merge_results(queries, &outcomes);
        let errors = collect_errors(&outcomes, queries, "query");
        Ok(json!({
            "queries": queries,
            "results": results,
            "succeeded": queries.len() - failed,
            "failed": failed,
            "errors": errors,
        })
        .to_string())
    }

    /// Fetches every URL at the same time, then applies the output budget in
    /// request order so truncation stays deterministic.
    async fn fetch_many(&self, urls: &[String]) -> Result<String, ToolError> {
        let started = Instant::now();
        let outcomes = join_all(urls.iter().enumerate().map(|(index, url)| async move {
            let url_started = Instant::now();
            let outcome = self.fetch_page(url).await;
            let elapsed = url_started.elapsed();
            telemetry::record(&format!("web.fetch.url.{index}"), elapsed);
            telemetry::record("web.fetch.url", elapsed);
            telemetry::increment(if outcome.is_ok() {
                "web.fetch.ok"
            } else {
                "web.fetch.failed"
            });
            outcome
        }))
        .await;
        telemetry::record("web.fetch", started.elapsed());
        let failed = outcomes.iter().filter(|outcome| outcome.is_err()).count();
        if failed == urls.len() {
            return Err(ToolError::Execution(format!(
                "web fetch failed for every url: {}",
                describe(&outcomes, urls, "url")
            )));
        }
        let mut pages = Vec::new();
        let mut errors = Vec::new();
        for (url, outcome) in urls.iter().zip(outcomes) {
            match outcome {
                Ok(page) => pages.push(page),
                Err(error) => errors.push(json!({"url": url, "error": error.to_string()})),
            }
        }
        apply_budget(&mut pages);
        Ok(json!({
            "pages": pages,
            "succeeded": pages.len(),
            "failed": failed,
            "errors": errors,
        })
        .to_string())
    }

    /// Retrieves one page: GET only, bounded body, HTML converted to Markdown.
    async fn fetch_page(&self, url: &str) -> Result<Page, ToolError> {
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
        let (title, content) = if content_type.starts_with("text/html") {
            (
                extract_title(&text),
                merge_blank_lines(&html2md::parse_html(&strip_noise(&text))),
            )
        } else {
            (None, merge_blank_lines(&text))
        };
        Ok(Page {
            url: final_url,
            title,
            content_type,
            content,
            truncated: false,
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    Search {
        #[serde(default)]
        queries: Vec<String>,
        #[serde(default)]
        query: Option<String>,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Fetch {
        #[serde(default)]
        urls: Vec<String>,
        #[serde(default)]
        url: Option<String>,
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
        "Search the web or fetch HTTP(S) pages as clean Markdown/text. Read-only GET requests. \
         search accepts 1-4 queries and runs them concurrently; fetch accepts 1-6 URLs and fetches \
         them concurrently. Prefer batching independent searches into one call. Prefer batching \
         independent page fetches into one call. Results are deduplicated by URL and partial \
         failures do not cancel successful results."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": ["search", "fetch"]},
                "queries": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": MAX_QUERIES, "description": "Search queries, executed concurrently."},
                "urls": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": MAX_FETCH_URLS, "description": "HTTP(S) URLs, fetched concurrently."},
                "query": {"type": "string", "description": "Single-query alias; prefer queries."},
                "url": {"type": "string", "description": "Single-URL alias; prefer urls."},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Results per query."}
            },
            "required": ["operation"],
            "additionalProperties": false
        })
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
            Input::Fetch { urls, url } => {
                let urls = normalize(urls, url, MAX_FETCH_URLS, "urls")?;
                self.fetch_many(&urls).await
            }
            Input::Search {
                queries,
                query,
                limit,
            } => {
                if !(1..=MAX_LIMIT).contains(&limit) {
                    return Err(ToolError::InvalidInput(format!(
                        "limit must be between 1 and {MAX_LIMIT}"
                    )));
                }
                let queries = normalize(queries, query, MAX_QUERIES, "queries")?;
                self.search_many(&queries, limit).await
            }
        }
    }
}

/// Trims, drops exact duplicates, and folds a legacy singular argument into the
/// list. Order is preserved so results stay stable.
fn normalize(
    values: Vec<String>,
    legacy: Option<String>,
    maximum: usize,
    field: &str,
) -> Result<Vec<String>, ToolError> {
    let mut normalized: Vec<String> = Vec::new();
    for value in values.into_iter().chain(legacy) {
        let value = value.trim().to_owned();
        if value.is_empty() {
            return Err(ToolError::InvalidInput(format!(
                "{field} must not contain empty values"
            )));
        }
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    if normalized.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "{field} requires 1-{maximum} values"
        )));
    }
    if normalized.len() > maximum {
        return Err(ToolError::InvalidInput(format!(
            "{field} accepts at most {maximum} values, got {}",
            normalized.len()
        )));
    }
    Ok(normalized)
}

/// Merges per-query results: first occurrence wins the position and the
/// highest-ranked copy of a URL is kept, later duplicates only add their query.
fn merge_results(
    queries: &[String],
    outcomes: &[Result<Vec<SearchResult>, ToolError>],
) -> Vec<SearchResult> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut merged: Vec<SearchResult> = Vec::new();
    for (query, outcome) in queries.iter().zip(outcomes) {
        let Ok(results) = outcome else {
            continue;
        };
        for result in results {
            if result.url.trim().is_empty() {
                continue;
            }
            let key = canonical_url(&result.url);
            if let Some(position) = seen.get(&key) {
                let existing = &mut merged[*position];
                if !existing.matched_queries.contains(query) {
                    existing.matched_queries.push(query.clone());
                }
            } else {
                seen.insert(key, merged.len());
                let mut result = result.clone();
                result.matched_queries = vec![query.clone()];
                merged.push(result);
            }
        }
    }
    merged
}

/// Canonical form used only for deduplication: fragment and tracking
/// parameters removed, trailing slash trimmed. Path case and the order of the
/// remaining query parameters are preserved so distinct pages stay distinct.
fn canonical_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let Ok(url) = reqwest::Url::parse(trimmed) else {
        return trimmed.trim_end_matches('/').to_owned();
    };
    let mut url = url;
    url.set_fragment(None);
    let retained: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| !is_tracking(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !retained.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (name, value) in &retained {
            pairs.append_pair(name, value);
        }
        pairs.finish();
    }
    let path = url.path().to_owned();
    if path.len() > 1 && path.ends_with('/') {
        url.set_path(path.trim_end_matches('/'));
    }
    url.to_string()
}

fn is_tracking(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    TRACKING_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || TRACKING_PARAMS.contains(&name.as_str())
}

fn collect_errors<T>(
    outcomes: &[Result<T, ToolError>],
    inputs: &[String],
    field: &str,
) -> Vec<Value> {
    outcomes
        .iter()
        .zip(inputs)
        .filter_map(|(outcome, input)| {
            outcome
                .as_ref()
                .err()
                .map(|error| json!({field: input, "error": error.to_string()}))
        })
        .collect()
}

fn describe<T>(outcomes: &[Result<T, ToolError>], inputs: &[String], field: &str) -> String {
    collect_errors(outcomes, inputs, field)
        .iter()
        .map(|entry| {
            format!(
                "{}: {}",
                entry[field].as_str().unwrap_or_default(),
                entry["error"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Allocates the output budget in request order: a page never returns more
/// than [`MAX_PAGE_CHARS`], and the call never returns more than
/// [`MAX_TOTAL_CHARS`]. Truncation is always reported.
fn apply_budget(pages: &mut [Page]) {
    let mut used = 0;
    for page in pages.iter_mut() {
        let remaining = MAX_TOTAL_CHARS.saturating_sub(used);
        let budget = MAX_PAGE_CHARS.min(remaining);
        if page.content.chars().count() > budget {
            page.content = page.content.chars().take(budget).collect();
            page.truncated = true;
        }
        used = used.saturating_add(page.content.chars().count());
    }
}

/// Removes comments and elements whose text is never content. An element
/// without its closing tag loses only the opening tag, so malformed markup
/// cannot swallow the rest of the page.
fn strip_noise(html: &str) -> String {
    let mut current = remove_comments(html);
    for tag in NOISE_ELEMENTS {
        current = remove_element(&current, tag);
    }
    current
}

fn remove_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(offset) = html[cursor..].find("<!--") {
        out.push_str(&html[cursor..cursor + offset]);
        let Some(end) = html[cursor + offset..].find("-->") else {
            return out;
        };
        cursor += offset + end + "-->".len();
    }
    out.push_str(&html[cursor..]);
    out
}

fn remove_element(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find(&open) {
        let start = cursor + offset;
        let after = start + open.len();
        out.push_str(&html[cursor..start]);
        let boundary = lower[after..]
            .chars()
            .next()
            .is_some_and(|c| c == '>' || c == '/' || c.is_ascii_whitespace());
        if !boundary {
            out.push_str(&html[start..after]);
            cursor = after;
            continue;
        }
        match lower[after..].find(&close) {
            Some(close_offset) => {
                let close_at = after + close_offset;
                match lower[close_at..].find('>') {
                    Some(end) => cursor = close_at + end + 1,
                    None => cursor = close_at,
                }
            }
            None => match lower[after..].find('>') {
                Some(end) => cursor = after + end + 1,
                None => cursor = lower.len(),
            },
        }
    }
    out.push_str(&html[cursor..]);
    out
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = start + lower[start..].find('>')? + 1;
    let close_start = open_end + lower[open_end..].find("</title")?;
    let raw = html[open_end..close_start]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title: String = decode_entities(&raw)
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect();
    (!title.is_empty()).then_some(title)
}

fn decode_entities(text: &str) -> String {
    let mut out = text
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    out = out.replace("&amp;", "&");
    out
}

/// Collapses whitespace noise: trailing blanks per line, runs of blank lines,
/// and non-breaking spaces. Indentation inside a line is preserved so code
/// blocks and tables stay intact.
fn merge_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in text.lines() {
        let line = line.replace('\u{a0}', " ");
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank_run = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_owned()
}
