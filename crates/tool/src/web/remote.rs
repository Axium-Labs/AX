//! Hosted, authorized anonymous MCP search. No cookies, paid keys or REST
//! endpoints are synthesized. Initialization/schema discovery is lazy and cached.
use super::{MAX_BYTES, SearchProvider, SearchResult, ToolError};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy)]
pub(super) enum RemoteKind {
    Exa,
    Parallel,
}

struct Session {
    protocol: String,
    id: Option<String>,
    schema: Value,
}
pub(super) struct RemoteSearch {
    client: reqwest::Client,
    url: String,
    kind: RemoteKind,
    session: tokio::sync::Mutex<Option<Arc<Session>>>,
    next_id: AtomicU64,
    conversation: String,
}
impl RemoteSearch {
    pub(super) fn new(client: reqwest::Client, url: String, kind: RemoteKind) -> Self {
        Self {
            client,
            url,
            kind,
            session: tokio::sync::Mutex::new(None),
            next_id: AtomicU64::new(1),
            conversation: uuid::Uuid::new_v4().to_string(),
        }
    }
    fn name(&self) -> &'static str {
        match self.kind {
            RemoteKind::Exa => "web_search_exa",
            RemoteKind::Parallel => "web_search",
        }
    }
    fn source(&self) -> &'static str {
        match self.kind {
            RemoteKind::Exa => "exa",
            RemoteKind::Parallel => "parallel",
        }
    }
    async fn rpc(
        &self,
        session: Option<&Session>,
        method: &str,
        params: Value,
        notification: bool,
    ) -> Result<(Value, Option<String>), ToolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut body = json!({"jsonrpc":"2.0","method":method,"params":params});
        if !notification {
            body["id"] = json!(id);
        }
        let mut request = self
            .client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .json(&body);
        if let Some(session) = session {
            request = request.header("MCP-Protocol-Version", &session.protocol);
            if let Some(id) = &session.id {
                request = request.header("Mcp-Session-Id", id);
            }
        }
        let http = request.send().await.map_err(|e| {
            ToolError::Execution(format!(
                "{}: {}",
                self.source(),
                if e.is_timeout() {
                    "request timeout"
                } else if e.is_connect() {
                    "connection/DNS/TLS/proxy failure"
                } else {
                    "request failed"
                }
            ))
        })?;
        if !http.status().is_success() {
            return Err(ToolError::SearchHttp {
                status: http.status().as_u16(),
                retry_after: model::retry::retry_after_header(http.headers()),
            });
        }
        let session_id = http
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if notification {
            return Ok((Value::Null, session_id));
        }
        let sse = http
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let mut stream = http.bytes_stream();
        let mut bytes = vec![];
        let mut total = 0;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| ToolError::Execution("MCP response read failed".into()))?;
            total += chunk.len();
            if total > MAX_BYTES {
                return Err(ToolError::Execution(
                    "MCP response exceeds size limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
            if sse {
                while let Some((end, size)) = delimiter(&bytes) {
                    let frame = std::str::from_utf8(&bytes[..end])
                        .map_err(|_| ToolError::Execution("invalid MCP UTF-8".into()))?;
                    let data = frame
                        .lines()
                        .filter_map(|s| s.strip_prefix("data:").map(str::trim_start))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !data.is_empty() {
                        let value: Value = serde_json::from_str(&data)
                            .map_err(|_| ToolError::Execution("invalid MCP SSE JSON".into()))?;
                        if value["id"] == id {
                            return Ok((rpc_result(&value)?, session_id));
                        }
                    }
                    bytes.drain(..end + size);
                }
            }
        }
        if sse {
            return Err(ToolError::Execution("truncated MCP response".into()));
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ToolError::Execution("invalid MCP JSON".into()))?;
        if value["id"] != id {
            return Err(ToolError::Execution("MCP response ID mismatch".into()));
        }
        Ok((rpc_result(&value)?, session_id))
    }
    async fn session(&self) -> Result<Arc<Session>, ToolError> {
        let mut cached = self.session.lock().await;
        if let Some(session) = &*cached {
            return Ok(session.clone());
        }
        let initialized_session: Result<Session,ToolError> = async {
            let (initialized,id) = self.rpc(None,"initialize",json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"ax","version":env!("CARGO_PKG_VERSION")}}),false).await?;
            let protocol = initialized["protocolVersion"].as_str().filter(|v| matches!(*v,"2025-03-26"|"2025-06-18"|"2024-11-05")).ok_or_else(|| ToolError::Execution("unsupported MCP protocol".into()))?.to_owned();
            let mut session = Session {protocol,id,schema:Value::Null};
            self.rpc(Some(&session),"notifications/initialized",json!({}),true).await?;
            let (catalog,_) = self.rpc(Some(&session),"tools/list",json!({}),false).await?;
            session.schema = catalog["tools"].as_array().and_then(|tools|tools.iter().find(|t|t["name"]==self.name())).map(|t|t["inputSchema"].clone()).ok_or_else(|| ToolError::Execution("MCP server has no expected search tool".into()))?;
            Ok(session)
        }.await;
        let session = Arc::new(initialized_session?);
        *cached = Some(session.clone());
        Ok(session)
    }
}
fn delimiter(bytes: &[u8]) -> Option<(usize, usize)> {
    (0..bytes.len()).find_map(|i| {
        if bytes[i..].starts_with(b"\r\n\r\n") {
            Some((i, 4))
        } else if bytes[i..].starts_with(b"\n\n") {
            Some((i, 2))
        } else {
            None
        }
    })
}
fn rpc_result(value: &Value) -> Result<Value, ToolError> {
    if value["jsonrpc"] != "2.0" || value.get("error").is_some() || value.get("result").is_none() {
        return Err(ToolError::Execution("MCP search protocol error".into()));
    }
    Ok(value["result"].clone())
}

#[async_trait]
impl SearchProvider for RemoteSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
        let session = self.session().await?;
        let props = &session.schema["properties"];
        let mut args = match self.kind {
            RemoteKind::Exa => json!({"query":query,"numResults":limit}),
            RemoteKind::Parallel => json!({"objective":query,"search_queries":[query]}),
        };
        if props.get("objective").is_some() {
            args["objective"] = json!(query);
        }
        if props.get("session_id").is_some() {
            args["session_id"] = json!(self.conversation);
        }
        // Schema changes fail closed instead of inventing arguments or bypassing auth.
        if let Some(required) = session.schema["required"].as_array()
            && required
                .iter()
                .filter_map(Value::as_str)
                .any(|name| args.get(name).is_none())
        {
            return Err(ToolError::Execution(
                "MCP search schema has unknown required arguments".into(),
            ));
        }
        let outcome = self
            .rpc(
                Some(&session),
                "tools/call",
                json!({"name":self.name(),"arguments":args}),
                false,
            )
            .await;
        if matches!(&outcome, Err(ToolError::SearchHttp { status: 404, .. })) {
            let mut cached = self.session.lock().await;
            if cached
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &session))
            {
                *cached = None;
            }
        }
        let (result, _) = outcome?;
        if result["isError"] == true {
            return Err(ToolError::Execution(format!(
                "{}: remote search tool error",
                self.source()
            )));
        }
        let mut results = vec![];
        let mut empty_result = result
            .pointer("/structuredContent/results")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        if let Some(data) = result.get("structuredContent") {
            structured(data, self.source(), &mut results)?;
        }
        if results.is_empty()
            && let Some(content) = result["content"].as_array()
        {
            for block in content {
                if block["type"] != "text" {
                    continue;
                }
                if let Some(text) = block["text"].as_str() {
                    if let Ok(data) = serde_json::from_str::<Value>(text) {
                        empty_result |= data["results"].as_array().is_some_and(Vec::is_empty);
                        structured(&data, self.source(), &mut results)?;
                    } else if matches!(self.kind, RemoteKind::Exa) {
                        exa_text(text, &mut results);
                    }
                }
            }
        }
        if results.is_empty() && !empty_result {
            return Err(ToolError::Execution(format!(
                "{}: no usable search results",
                self.source()
            )));
        }
        results.truncate(limit);
        Ok(results)
    }
}
fn structured(
    value: &Value,
    source: &str,
    results: &mut Vec<SearchResult>,
) -> Result<(), ToolError> {
    let entries = value["results"]
        .as_array()
        .ok_or_else(|| ToolError::Execution("MCP search missing results array".into()))?;
    for entry in entries {
        let Some(url) = entry["url"].as_str().filter(|u| {
            reqwest::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
        }) else {
            continue;
        };
        let title = entry["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(url);
        let excerpts = entry["excerpts"].as_array().map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        });
        let text = excerpts
            .as_deref()
            .or_else(|| entry["text"].as_str())
            .unwrap_or_default();
        let mut result = SearchResult::new(
            title,
            url,
            text.chars().take(6000).collect::<String>(),
            source,
        );
        if let Some(date) = entry.get("publish_date") {
            result.evidence.push(json!({"publish_date":date}));
        }
        results.push(result);
    }
    Ok(())
}
fn exa_text(text: &str, results: &mut Vec<SearchResult>) {
    // Parse the hosted server's documented result record, never a search HTML page.
    for record in text.split("\n\n---\n\n") {
        let record = record.strip_prefix("Title: ").unwrap_or(record);
        let Some((title, rest)) = record.split_once('\n') else {
            continue;
        };
        let Some(rest) = rest.strip_prefix("URL: ") else {
            continue;
        };
        let (url, snippet) = rest.split_once('\n').unwrap_or((rest, ""));
        if !reqwest::Url::parse(url.trim()).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
            continue;
        }
        results.push(SearchResult::new(
            title.trim(),
            url.trim(),
            snippet.chars().take(6000).collect::<String>(),
            "exa",
        ));
    }
}
