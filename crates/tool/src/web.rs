//! Side-effect-free web search and bounded page retrieval.
//!
//! One tool call may carry several independent queries or URLs. They are
//! executed concurrently, merged, deduplicated and returned as one structured
//! result so the agent loop needs fewer model round trips.
use crate::{Capability, SafetyLevel, Tool, ToolError, telemetry};
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, error::Error, sync::Arc, time::Instant};

mod network;
mod providers;
pub use providers::SearchConfig;

/// Diagnostic category; these labels never change request policy.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FetchErrorKind {
    Dns,
    Connect,
    Timeout,
    Tls,
    Proxy,
    Redirect,
    HttpStatus,
    Unknown,
}

/// A failed URL, including every source from the outer error to the root cause.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FetchError {
    pub url: String,
    pub error: String,
    pub kind: FetchErrorKind,
    pub reason: String,
    pub source_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
}

impl FetchError {
    fn from_error(url: &str, error: &(dyn Error + 'static)) -> Self {
        let mut source_chain = vec![error.to_string()];
        let mut source = error.source();
        while let Some(cause) = source {
            source_chain.push(cause.to_string());
            source = cause.source();
        }
        let request = error.downcast_ref::<reqwest::Error>();
        let status_code = request.and_then(reqwest::Error::status).map(|s| s.as_u16());
        let timeout = request.is_some_and(reqwest::Error::is_timeout);
        let connect = request.is_some_and(reqwest::Error::is_connect);
        let redirect = request.is_some_and(reqwest::Error::is_redirect);
        // reqwest exposes typed flags for status/timeout/connect/redirect, but
        // not DNS, TLS or proxy. Inspect sources only, never the URL-bearing
        // outer message, so host names cannot determine the classification.
        let causes = source_chain
            .iter()
            .skip(1)
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("\n");
        let kind = classify_fetch_error(status_code, timeout, connect, redirect, &causes);
        let reason = match kind {
            FetchErrorKind::Dns => "dns resolution failed".into(),
            FetchErrorKind::Connect => "connection failed".into(),
            FetchErrorKind::Timeout if connect => "connect timeout".into(),
            FetchErrorKind::Timeout => "request timeout".into(),
            FetchErrorKind::Tls => "tls handshake failed".into(),
            FetchErrorKind::Proxy => "proxy failed".into(),
            FetchErrorKind::Redirect => "redirect failed".into(),
            FetchErrorKind::HttpStatus => format!("HTTP {}", status_code.unwrap_or_default()),
            FetchErrorKind::Unknown if request.is_none() => source_chain[0].clone(),
            FetchErrorKind::Unknown => "request failed".into(),
        };
        Self {
            url: url.into(),
            error: source_chain.join(": "),
            kind,
            reason,
            source_chain,
            status_code,
        }
    }
}

fn classify_fetch_error(
    status: Option<u16>,
    timeout: bool,
    connect: bool,
    redirect: bool,
    causes: &str,
) -> FetchErrorKind {
    if status.is_some() {
        return FetchErrorKind::HttpStatus;
    }
    if redirect {
        return FetchErrorKind::Redirect;
    }
    if timeout {
        return FetchErrorKind::Timeout;
    }
    if ["proxy", "tunnel", "socks"]
        .iter()
        .any(|s| causes.contains(s))
    {
        return FetchErrorKind::Proxy;
    }
    if [
        "dns",
        "resolve",
        "resolution",
        "lookup address",
        "name or service not known",
        "no such host",
        "nodename nor servname",
    ]
    .iter()
    .any(|s| causes.contains(s))
    {
        return FetchErrorKind::Dns;
    }
    if [
        "tls",
        "ssl",
        "certificate",
        "cert error",
        "handshake",
        "invalid peer",
        "unknownissuer",
        "received corrupt message",
        "peer misbehaved",
        "peer is incompatible",
    ]
    .iter()
    .any(|s| causes.contains(s))
    {
        return FetchErrorKind::Tls;
    }
    if connect {
        return FetchErrorKind::Connect;
    }
    FetchErrorKind::Unknown
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn classification_precedence_and_unknown_fallback() {
        let cases = [
            (
                Some(403),
                false,
                false,
                false,
                "",
                FetchErrorKind::HttpStatus,
            ),
            (
                None,
                false,
                false,
                true,
                "dns error",
                FetchErrorKind::Redirect,
            ),
            (None, true, true, false, "proxy", FetchErrorKind::Timeout),
            (
                None,
                false,
                true,
                false,
                "proxy tunnel failed: dns error",
                FetchErrorKind::Proxy,
            ),
            (None, false, true, false, "dns error", FetchErrorKind::Dns),
            (
                None,
                false,
                true,
                false,
                "invalid peer certificate: UnknownIssuer",
                FetchErrorKind::Tls,
            ),
            (
                None,
                false,
                true,
                false,
                "tcp connect error",
                FetchErrorKind::Connect,
            ),
            (
                None,
                false,
                false,
                false,
                "body decode error",
                FetchErrorKind::Unknown,
            ),
        ];
        for (status, timeout, connect, redirect, causes, expected) in cases {
            assert_eq!(
                classify_fetch_error(
                    status,
                    timeout,
                    connect,
                    redirect,
                    &causes.to_ascii_lowercase()
                ),
                expected
            );
        }
    }
}

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
    search_config: SearchConfig,
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
        Self {
            client: network::client(),
            search: None,
            search_config: SearchConfig::from_env(),
        }
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

    #[must_use]
    pub fn with_search_config(mut self, config: SearchConfig) -> Self {
        self.search_config = config;
        self.search = None;
        self
    }

    /// Runs every query at the same time and merges the results.
    async fn search_many(
        &self,
        queries: &[String],
        limit: usize,
        target: usize,
    ) -> Result<String, ToolError> {
        let provider = self.search.clone().unwrap_or_else(|| {
            Arc::new(providers::BuiltinSearch {
                client: self.client.clone(),
                config: self.search_config.clone(),
            })
        });
        let started = Instant::now();
        let requests = queries
            .iter()
            .enumerate()
            .map(|(index, query)| {
                let provider = Arc::clone(&provider);
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
                    (index, outcome)
                }
            })
            .collect::<Vec<_>>();
        let mut pending = stream::iter(requests).buffer_unordered(MAX_QUERIES);
        let mut outcomes: Vec<_> = (0..queries.len()).map(|_| None).collect();
        while let Some((index, outcome)) = pending.next().await {
            outcomes[index] = Some(outcome);
            if merge_completed(queries, &outcomes).len() >= target {
                break;
            }
        }
        drop(pending);
        telemetry::record("web.search", started.elapsed());
        let cancelled: Vec<_> = queries
            .iter()
            .zip(&outcomes)
            .filter_map(|(query, result)| result.is_none().then_some(query))
            .collect();
        for _ in &cancelled {
            telemetry::increment("web.search.cancelled");
        }
        let completed = outcomes.iter().flatten().count();
        let failed = outcomes
            .iter()
            .flatten()
            .filter(|outcome| outcome.is_err())
            .count();
        let errors: Vec<_> = queries
            .iter()
            .zip(&outcomes)
            .filter_map(|(query, outcome)| {
                outcome
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .map(|error| json!({"query": query, "error": error.to_string()}))
            })
            .collect();
        if failed == queries.len() {
            return Err(ToolError::Execution(format!(
                "web search failed for every query: {}",
                errors
                    .iter()
                    .map(|error| format!(
                        "{}: {}",
                        error["query"].as_str().unwrap_or_default(),
                        error["error"].as_str().unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        let mut results = merge_completed(queries, &outcomes);
        results.truncate(target);
        Ok(json!({
            "queries": queries,
            "results": results,
            "succeeded": completed - failed,
            "failed": failed,
            "errors": errors,
            "cancelled_queries": cancelled,
        })
        .to_string())
    }

    /// Fetches every URL at the same time, then applies the output budget in
    /// request order so truncation stays deterministic.
    async fn fetch_many(&self, urls: &[String], target: usize) -> Result<String, ToolError> {
        let started = Instant::now();
        let requests = urls
            .iter()
            .enumerate()
            .map(|(index, url)| async move {
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
                (index, outcome)
            })
            .collect::<Vec<_>>();
        let mut pending = stream::iter(requests).buffer_unordered(network::FETCH_CONCURRENCY);
        let mut outcomes: Vec<_> = (0..urls.len()).map(|_| None).collect();
        let mut succeeded = 0;
        while let Some((index, outcome)) = pending.next().await {
            if outcome.is_ok() {
                succeeded += 1;
            }
            outcomes[index] = Some(outcome);
            if succeeded >= target {
                break;
            }
        }
        drop(pending);
        telemetry::record("web.fetch", started.elapsed());
        let cancelled: Vec<_> = urls
            .iter()
            .zip(&outcomes)
            .filter_map(|(url, outcome)| outcome.is_none().then_some(url))
            .collect();
        for _ in &cancelled {
            telemetry::increment("web.fetch.cancelled");
        }
        let failed = outcomes
            .iter()
            .flatten()
            .filter(|outcome| outcome.is_err())
            .count();
        if failed == urls.len() {
            return Err(ToolError::WebFetch(
                outcomes
                    .into_iter()
                    .flatten()
                    .filter_map(Result::err)
                    .collect(),
            ));
        }
        let mut pages = Vec::new();
        let mut errors = Vec::new();
        for outcome in outcomes.into_iter().flatten() {
            match outcome {
                Ok(page) => pages.push(page),
                Err(error) => errors.push(error),
            }
        }
        apply_budget(&mut pages);
        Ok(json!({
            "pages": pages,
            "succeeded": pages.len(),
            "failed": failed,
            "errors": errors,
            "cancelled_urls": cancelled,
        })
        .to_string())
    }

    /// Retrieves one page: GET only, bounded body, HTML converted to Markdown.
    async fn fetch_page(&self, url: &str) -> Result<Page, FetchError> {
        let parsed = reqwest::Url::parse(url).map_err(|e| FetchError::from_error(url, &e))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(FetchError::from_error(
                url,
                &ToolError::InvalidInput("web fetch only accepts HTTP(S) URLs".into()),
            ));
        }
        let body = network::get(self.client.get(parsed), url, true).await?;
        let content_type = body.content_type;
        let final_url = body.url;
        let text = String::from_utf8(body.bytes).map_err(|e| FetchError::from_error(url, &e))?;
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
        #[serde(default)]
        target_results: Option<usize>,
    },
    Fetch {
        #[serde(default)]
        urls: Vec<String>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        target_pages: Option<usize>,
    },
}
const fn default_limit() -> usize {
    5
}

#[async_trait]
impl Tool for WebTool {
    fn resources(&self, _input: &Value) -> Vec<crate::ResourceAccess> {
        vec![crate::ResourceAccess::read(crate::Resource::Named(
            "web-network".into(),
        ))]
    }
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
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Results per query; also the default total result target."},
                "target_results": {"type":"integer", "minimum":1, "maximum":MAX_LIMIT * MAX_QUERIES, "description":"Cancel remaining queries after this many distinct valid results. Defaults to limit."},
                "target_pages": {"type":"integer", "minimum":1, "maximum":MAX_FETCH_URLS, "description":"Cancel remaining fetches after this many successful pages. Defaults to all requested URLs."}
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
            Input::Fetch {
                urls,
                url,
                target_pages,
            } => {
                let urls = normalize(urls, url, MAX_FETCH_URLS, "urls")?;
                let target = validate_target(
                    target_pages.unwrap_or(urls.len()),
                    urls.len(),
                    "target_pages",
                )?;
                self.fetch_many(&urls, target).await
            }
            Input::Search {
                queries,
                query,
                limit,
                target_results,
            } => {
                if !(1..=MAX_LIMIT).contains(&limit) {
                    return Err(ToolError::InvalidInput(format!(
                        "limit must be between 1 and {MAX_LIMIT}"
                    )));
                }
                let queries = normalize(queries, query, MAX_QUERIES, "queries")?;
                let target = validate_target(
                    target_results.unwrap_or(limit),
                    limit * queries.len(),
                    "target_results",
                )?;
                self.search_many(&queries, limit, target).await
            }
        }
    }
}

/// Trims, drops exact duplicates, and folds a legacy singular argument into the
/// list. Order is preserved so results stay stable.
fn validate_target(target: usize, maximum: usize, field: &str) -> Result<usize, ToolError> {
    if target == 0 || target > maximum {
        return Err(ToolError::InvalidInput(format!(
            "{field} must be between 1 and {maximum}"
        )));
    }
    Ok(target)
}

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

fn merge_completed(
    queries: &[String],
    outcomes: &[Option<Result<Vec<SearchResult>, ToolError>>],
) -> Vec<SearchResult> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut merged: Vec<SearchResult> = Vec::new();
    for (query, outcome) in queries.iter().zip(outcomes) {
        let Some(Ok(results)) = outcome else { continue };
        for result in results {
            if !reqwest::Url::parse(&result.url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
                || result.title.trim().is_empty()
            {
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
