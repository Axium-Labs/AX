use std::collections::BTreeMap;

use async_trait::async_trait;
use reqwest::{
    Client,
    header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use serde_json::Value;

use super::Transport;
use crate::McpError;

pub(crate) struct HttpTransport {
    client: Client,
    url: String,
    headers: HeaderMap,
    protocol_version: String,
    session_id: Option<HeaderValue>,
}

impl HttpTransport {
    pub(crate) fn new(
        url: &str,
        headers: &BTreeMap<String, String>,
        protocol_version: &str,
    ) -> Result<Self, McpError> {
        if url.trim().is_empty() {
            return Err(McpError::InvalidTransport(
                "HTTP URL must not be empty".to_owned(),
            ));
        }
        let mut parsed_headers = HeaderMap::new();
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| McpError::InvalidTransport(error.to_string()))?;
            let value = HeaderValue::from_str(value)
                .map_err(|error| McpError::InvalidTransport(error.to_string()))?;
            parsed_headers.insert(name, value);
        }
        Ok(Self {
            client: Client::new(),
            url: url.to_owned(),
            headers: parsed_headers,
            protocol_version: protocol_version.to_owned(),
            session_id: None,
        })
    }

    async fn post(
        &mut self,
        payload: &Value,
        method: &str,
        name: Option<&str>,
    ) -> Result<reqwest::Response, McpError> {
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", &self.protocol_version)
            .header("Mcp-Method", method)
            .json(payload);
        if let Some(session_id) = &self.session_id {
            request = request.header("Mcp-Session-Id", session_id);
        }
        if let Some(name) = name {
            request = request.header("Mcp-Name", name);
        }
        let response = request
            .send()
            .await
            .map_err(|error| McpError::Transport(error.to_string()))?
            .error_for_status()
            .map_err(|error| McpError::Transport(error.to_string()))?;
        if let Some(session_id) = response.headers().get("Mcp-Session-Id") {
            self.session_id = Some(session_id.clone());
        }
        Ok(response)
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn request(
        &mut self,
        payload: &Value,
        method: &str,
        name: Option<&str>,
    ) -> Result<Value, McpError> {
        let response = self.post(payload, method, name).await?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        if content_type.contains("text/event-stream") {
            let body = response
                .text()
                .await
                .map_err(|error| McpError::Transport(error.to_string()))?;
            parse_sse_response(&body, payload.get("id"))
        } else {
            response
                .json()
                .await
                .map_err(|error| McpError::InvalidResponse(error.to_string()))
        }
    }

    async fn notify(&mut self, payload: &Value, method: &str) -> Result<(), McpError> {
        self.post(payload, method, None).await?;
        Ok(())
    }
}

fn parse_sse_response(body: &str, expected_id: Option<&Value>) -> Result<Value, McpError> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .find(|value| expected_id.is_none() || value.get("id") == expected_id)
        .ok_or_else(|| McpError::InvalidResponse("SSE response contained no JSON data".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_from_sse_data() {
        let value = parse_sse_response(
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notice\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1}\n\n",
            Some(&serde_json::json!(1)),
        )
        .expect("SSE response should parse");
        assert_eq!(value["id"], 1);
    }
}
