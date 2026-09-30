//! Optional Brave search and keyless `DuckDuckGo` HTML fallback.
use super::{SearchProvider, SearchResult, ToolError, network, telemetry};
use async_trait::async_trait;
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde_json::Value;

/// Built-in provider configuration. Endpoints may be overridden by embedders
/// and tests; requests still share the `WebTool`'s HTTP client.
#[derive(Clone, Debug)]
pub struct SearchConfig {
    pub brave_api_key: Option<String>,
    pub brave_url: String,
    pub duckduckgo_url: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            brave_api_key: None,
            brave_url: "https://api.search.brave.com/res/v1/web/search".into(),
            duckduckgo_url: "https://html.duckduckgo.com/html/".into(),
        }
    }
}

impl SearchConfig {
    pub(super) fn from_env() -> Self {
        Self {
            brave_api_key: std::env::var("BRAVE_SEARCH_API_KEY")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            ..Self::default()
        }
    }
}

pub(super) struct BuiltinSearch {
    pub client: reqwest::Client,
    pub config: SearchConfig,
}

#[async_trait]
impl SearchProvider for BuiltinSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let mut brave_error = None;
        if let Some(key) = self
            .config
            .brave_api_key
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            let brave = BraveSearch {
                client: &self.client,
                url: &self.config.brave_url,
                key,
            };
            match brave.search(query, limit).await {
                Ok(results) if !results.is_empty() => return Ok(results),
                Ok(_) => telemetry::increment("web.search.brave.empty"),
                Err(error) => {
                    telemetry::increment("web.search.brave.failed");
                    brave_error = Some(error);
                }
            }
            telemetry::increment("web.search.fallback");
        }
        let duckduckgo = DuckDuckGoSearch {
            client: &self.client,
            url: &self.config.duckduckgo_url,
        };
        duckduckgo
            .search(query, limit)
            .await
            .map_err(|error| match brave_error {
                Some(brave) => ToolError::Execution(format!("Brave: {brave}; DuckDuckGo: {error}")),
                None => error,
            })
    }
}

struct BraveSearch<'a> {
    client: &'a reqwest::Client,
    url: &'a str,
    key: &'a str,
}

#[async_trait]
impl SearchProvider for BraveSearch<'_> {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let body = network::get(
            self.client
                .get(self.url)
                .header("X-Subscription-Token", self.key)
                .query(&[("q", query), ("count", &limit.to_string())]),
            self.url,
            false,
        )
        .await
        .map_err(|error| ToolError::Execution(error.error))?;
        let value: Value = serde_json::from_slice(&body.bytes)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(value
            .pointer("/web/results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|entry| {
                valid_url(entry["url"].as_str().unwrap_or_default()).is_some()
                    && !entry["title"]
                        .as_str()
                        .unwrap_or_default()
                        .trim()
                        .is_empty()
            })
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

struct DuckDuckGoSearch<'a> {
    client: &'a reqwest::Client,
    url: &'a str,
}

#[async_trait]
impl SearchProvider for DuckDuckGoSearch<'_> {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let body = network::get(
            self.client.get(self.url).query(&[("q", query)]),
            self.url,
            true,
        )
        .await
        .map_err(|error| ToolError::Execution(error.error))?;
        let html = String::from_utf8(body.bytes)
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        parse_duckduckgo(&html, limit)
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_results_decode_entities_redirects_and_exclude_ads() {
        let html = r#"<div class="result result--ad"><a class="result__a" href="https://ad.test">Ad</a></div>
        <div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.test%2Fdoc%3Fa%3D1%26b%3D2&amp;rut=abc">Title &amp; <b>More</b></a><a class="result__snippet">Body &#x4e2d; &amp; info</a></div>
        <div class="result"><a class="result__a" href="javascript:alert(1)">bad</a></div>"#;
        let results = parse_duckduckgo(html, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Title & More");
        assert_eq!(results[0].url, "https://example.test/doc?a=1&b=2");
        assert_eq!(results[0].snippet, "Body 中 & info");
        assert!(parse_duckduckgo("<form id='challenge'>captcha</form>", 5).is_err());
        assert!(
            parse_duckduckgo("<div class='no-results'>No results</div>", 5)
                .unwrap()
                .is_empty()
        );
    }
}
