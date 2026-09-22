use std::{collections::BTreeMap, fs, path::Path};

use serde::Deserialize;

use crate::{McpError, client::CURRENT_PROTOCOL_VERSION};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: BTreeMap<String, ServerConfig>,
}

impl McpConfig {
    /// Reads server metadata without opening any transport.
    /// A missing configuration file produces an empty registry.
    ///
    /// # Errors
    ///
    /// Returns an error when an existing file cannot be read or parsed.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, McpError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self {
                servers: BTreeMap::new(),
            });
        }
        let contents = fs::read_to_string(path).map_err(|source| McpError::ConfigIo {
            path: path.to_owned(),
            source,
        })?;
        toml::from_str(&contents).map_err(|source| McpError::ConfigParse {
            path: path.to_owned(),
            source,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default = "current_protocol")]
    pub protocol_version: String,
    #[serde(default = "default_timeout")]
    pub request_timeout_secs: u64,
    #[serde(flatten)]
    pub transport: TransportConfig,
}

const fn enabled() -> bool {
    true
}

fn current_protocol() -> String {
    CURRENT_PROTOCOL_VERSION.to_owned()
}

const fn default_timeout() -> u64 {
    30
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    #[serde(rename = "websocket")]
    WebSocket {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_transport_kinds_without_connecting() {
        let config: McpConfig = toml::from_str(
            r#"
                [servers.local]
                transport = "stdio"
                command = "server"
                args = ["--stdio"]

                [servers.remote]
                transport = "http"
                url = "https://example.test/mcp"

                [servers.custom]
                transport = "websocket"
                url = "wss://example.test/mcp"
            "#,
        )
        .expect("configuration should parse");

        assert_eq!(config.servers.len(), 3);
        assert!(matches!(
            config.servers["local"].transport,
            TransportConfig::Stdio { .. }
        ));
        assert_eq!(
            config.servers["remote"].protocol_version,
            CURRENT_PROTOCOL_VERSION
        );
    }
}
