use std::collections::BTreeMap;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use super::Transport;
use crate::McpError;

pub(crate) struct WebSocketTransport {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl WebSocketTransport {
    pub(crate) async fn connect(
        url: &str,
        headers: &BTreeMap<String, String>,
    ) -> Result<Self, McpError> {
        let mut request = url
            .into_client_request()
            .map_err(|error| McpError::InvalidTransport(error.to_string()))?;
        for (name, value) in headers {
            let name = name
                .parse::<tokio_tungstenite::tungstenite::http::HeaderName>()
                .map_err(|error| McpError::InvalidTransport(error.to_string()))?;
            let value = value
                .parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
                .map_err(|error| McpError::InvalidTransport(error.to_string()))?;
            request.headers_mut().insert(name, value);
        }
        let (stream, _) = connect_async(request)
            .await
            .map_err(|error| McpError::Transport(error.to_string()))?;
        Ok(Self { stream })
    }

    async fn send(&mut self, payload: &Value) -> Result<(), McpError> {
        self.stream
            .send(Message::Text(payload.to_string().into()))
            .await
            .map_err(|error| McpError::Transport(error.to_string()))
    }
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn request(
        &mut self,
        payload: &Value,
        _method: &str,
        _name: Option<&str>,
    ) -> Result<Value, McpError> {
        self.send(payload).await?;
        let expected_id = payload.get("id").cloned();
        while let Some(message) = self.stream.next().await {
            let message = message.map_err(|error| McpError::Transport(error.to_string()))?;
            let response = match message {
                Message::Text(text) => serde_json::from_str::<Value>(&text),
                Message::Binary(bytes) => serde_json::from_slice::<Value>(&bytes),
                Message::Close(_) => {
                    return Err(McpError::Transport(
                        "WebSocket server closed the connection".to_owned(),
                    ));
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
            .map_err(|error| McpError::InvalidResponse(error.to_string()))?;
            if response.get("id") == expected_id.as_ref() {
                return Ok(response);
            }
        }
        Err(McpError::Transport(
            "WebSocket response stream ended".to_owned(),
        ))
    }

    async fn notify(&mut self, payload: &Value, _method: &str) -> Result<(), McpError> {
        self.send(payload).await
    }
}
