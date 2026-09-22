use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    McpError, ServerConfig,
    transport::{self, Transport},
};

pub const CURRENT_PROTOCOL_VERSION: &str = "2026-07-28";

const CLIENT_NAME: &str = "ax";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    #[serde(default)]
    pub content: Vec<Value>,
    #[serde(default)]
    pub structured_content: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolListPage {
    #[serde(default)]
    tools: Vec<McpTool>,
    next_cursor: Option<String>,
}

pub struct McpClient {
    server_name: String,
    protocol_version: String,
    transport: Box<dyn Transport>,
    next_id: u64,
    request_timeout: Duration,
}

impl McpClient {
    /// Connects one configured server and performs the legacy handshake only
    /// when its configured protocol version predates `2026-07-28`.
    ///
    /// # Errors
    ///
    /// Returns an error when transport creation or legacy initialization fails.
    pub async fn connect(name: impl Into<String>, config: &ServerConfig) -> Result<Self, McpError> {
        let server_name = name.into();
        let transport = transport::connect(&config.transport, &config.protocol_version).await?;
        let mut client = Self {
            server_name,
            protocol_version: config.protocol_version.clone(),
            transport,
            next_id: 1,
            request_timeout: Duration::from_secs(config.request_timeout_secs.max(1)),
        };
        if !client.is_modern() {
            client.initialize_legacy().await?;
        }
        Ok(client)
    }

    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Discovers every page of tools exposed by this server.
    ///
    /// # Errors
    ///
    /// Returns an error for transport failures, protocol errors, malformed
    /// results, or a server that returns more than 100 cursor pages.
    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
            let result = self.request("tools/list", params, None).await?;
            let page = serde_json::from_value::<ToolListPage>(result)
                .map_err(|error| McpError::InvalidResponse(error.to_string()))?;
            tools.extend(page.tools);
            let Some(next_cursor) = page.next_cursor else {
                return Ok(tools);
            };
            cursor = Some(next_cursor);
        }
        Err(McpError::InvalidResponse(
            "tools/list exceeded 100 pages".to_owned(),
        ))
    }

    /// Invokes one remote MCP tool.
    ///
    /// # Errors
    ///
    /// Returns an error when transport, protocol, or result decoding fails.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult, McpError> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                Some(name),
            )
            .await?;
        serde_json::from_value(result).map_err(|error| McpError::InvalidResponse(error.to_string()))
    }

    async fn initialize_legacy(&mut self) -> Result<(), McpError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": self.protocol_version,
                "capabilities": {},
                "clientInfo": { "name": CLIENT_NAME, "version": CLIENT_VERSION }
            }),
            None,
        )
        .await?;
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.transport
            .notify(&notification, "notifications/initialized")
            .await
    }

    async fn request(
        &mut self,
        method: &str,
        mut params: Value,
        name: Option<&str>,
    ) -> Result<Value, McpError> {
        if self.is_modern() {
            attach_modern_metadata(&mut params)?;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let response = tokio::time::timeout(
            self.request_timeout,
            self.transport.request(&payload, method, name),
        )
        .await
        .map_err(|_| {
            McpError::Transport(format!(
                "request to '{}' timed out after {} seconds",
                self.server_name,
                self.request_timeout.as_secs()
            ))
        })??;
        decode_response(&response, id)
    }

    fn is_modern(&self) -> bool {
        self.protocol_version.as_str() >= CURRENT_PROTOCOL_VERSION
    }
}

fn attach_modern_metadata(params: &mut Value) -> Result<(), McpError> {
    let object = params.as_object_mut().ok_or_else(|| {
        McpError::InvalidResponse("request params must be a JSON object".to_owned())
    })?;
    object.insert(
        "_meta".to_owned(),
        json!({
            "io.modelcontextprotocol/protocolVersion": CURRENT_PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientInfo": {
                "name": CLIENT_NAME,
                "version": CLIENT_VERSION
            },
            "io.modelcontextprotocol/clientCapabilities": {}
        }),
    );
    Ok(())
}

fn decode_response(response: &Value, expected_id: u64) -> Result<Value, McpError> {
    if response.get("id").and_then(Value::as_u64) != Some(expected_id) {
        return Err(McpError::InvalidResponse(format!(
            "response id does not match request {expected_id}"
        )));
    }
    if let Some(error) = response.get("error") {
        return Err(McpError::Rpc {
            code: error.get("code").and_then(Value::as_i64).unwrap_or(-1),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown MCP error")
                .to_owned(),
        });
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::InvalidResponse("response has no result".to_owned()))
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Arc};

    use async_trait::async_trait;
    use tokio::sync::Mutex;

    use super::*;

    struct MockTransport {
        responses: VecDeque<Value>,
        requests: Arc<Mutex<Vec<Value>>>,
    }

    #[async_trait]
    impl Transport for MockTransport {
        async fn request(
            &mut self,
            payload: &Value,
            _method: &str,
            _name: Option<&str>,
        ) -> Result<Value, McpError> {
            self.requests.lock().await.push(payload.clone());
            self.responses
                .pop_front()
                .ok_or_else(|| McpError::Transport("mock responses exhausted".to_owned()))
        }

        async fn notify(&mut self, _payload: &Value, _method: &str) -> Result<(), McpError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn discovers_paginated_tools_with_modern_metadata() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = MockTransport {
            responses: VecDeque::from([
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": {
                        "tools": [{"name":"search","inputSchema":{"type":"object"}}],
                        "nextCursor": "page-2"
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "result": {
                        "tools": [{"name":"fetch","inputSchema":{"type":"object"}}]
                    }
                }),
            ]),
            requests: Arc::clone(&requests),
        };
        let mut client = McpClient {
            server_name: "mock".to_owned(),
            protocol_version: CURRENT_PROTOCOL_VERSION.to_owned(),
            transport: Box::new(transport),
            next_id: 1,
            request_timeout: Duration::from_secs(1),
        };

        let tools = client
            .list_tools()
            .await
            .expect("tools should be discovered");

        assert_eq!(tools.len(), 2);
        let requests = requests.lock().await;
        assert_eq!(requests[1]["params"]["cursor"], "page-2");
        assert_eq!(
            requests[0]["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            CURRENT_PROTOCOL_VERSION
        );
    }
}
