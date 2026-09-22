//! Lazy Model Context Protocol client runtime.
//!
//! Configuration loading creates no processes or network connections. A server
//! transport is constructed only when that named server is first used.

mod bridge;
mod client;
mod config;
mod manager;
mod transport;

use std::path::PathBuf;

use thiserror::Error;

pub use bridge::{McpToolProxy, discover_tool_proxies};
pub use client::{CURRENT_PROTOCOL_VERSION, McpClient, McpTool, ToolCallResult};
pub use config::{McpConfig, ServerConfig, TransportConfig};
pub use manager::{McpManager, ServerStatus};

#[derive(Debug, Error)]
pub enum McpError {
    #[error("failed to access MCP configuration {path}: {source}")]
    ConfigIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid MCP configuration {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("unknown MCP server: {0}")]
    UnknownServer(String),
    #[error("MCP server '{0}' is disabled")]
    DisabledServer(String),
    #[error("invalid MCP transport configuration: {0}")]
    InvalidTransport(String),
    #[error("MCP transport error: {0}")]
    Transport(String),
    #[error("MCP protocol error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("MCP response is invalid: {0}")]
    InvalidResponse(String),
}
