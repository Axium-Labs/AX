mod http;
mod stdio;
mod websocket;

use async_trait::async_trait;
use serde_json::Value;

use crate::{McpError, TransportConfig};

pub(crate) use http::HttpTransport;
pub(crate) use stdio::StdioTransport;
pub(crate) use websocket::WebSocketTransport;

#[async_trait]
pub(crate) trait Transport: Send {
    async fn request(
        &mut self,
        payload: &Value,
        method: &str,
        name: Option<&str>,
    ) -> Result<Value, McpError>;

    async fn notify(&mut self, payload: &Value, method: &str) -> Result<(), McpError>;
}

pub(crate) async fn connect(
    config: &TransportConfig,
    protocol_version: &str,
) -> Result<Box<dyn Transport>, McpError> {
    match config {
        TransportConfig::Stdio {
            command,
            args,
            env,
            cwd,
        } => Ok(Box::new(StdioTransport::connect(
            command,
            args,
            env,
            cwd.as_deref(),
        )?)),
        TransportConfig::Http { url, headers } => Ok(Box::new(HttpTransport::new(
            url,
            headers,
            protocol_version,
        )?)),
        TransportConfig::WebSocket { url, headers } => {
            Ok(Box::new(WebSocketTransport::connect(url, headers).await?))
        }
    }
}
