use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::{McpClient, McpConfig, McpError, McpTool, ServerConfig, ToolCallResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub enabled: bool,
    pub connected: bool,
    pub transport: &'static str,
    pub protocol_version: String,
}

pub struct McpManager {
    configs: BTreeMap<String, ServerConfig>,
    clients: HashMap<String, McpClient>,
}

impl McpManager {
    #[must_use]
    pub fn new(config: McpConfig) -> Self {
        Self {
            configs: config.servers,
            clients: HashMap::new(),
        }
    }

    #[must_use]
    pub fn connected_server_count(&self) -> usize {
        self.clients.len()
    }

    #[must_use]
    pub fn statuses(&self) -> Vec<ServerStatus> {
        self.configs
            .iter()
            .map(|(name, config)| ServerStatus {
                name: name.clone(),
                enabled: config.enabled,
                connected: self.clients.contains_key(name),
                transport: transport_name(config),
                protocol_version: config.protocol_version.clone(),
            })
            .collect()
    }

    /// Metadata-only discovery. Never starts a process or opens a connection.
    #[must_use]
    pub fn capability_catalog(&self) -> Value {
        serde_json::json!(self.configs.iter().map(|(name,config)| serde_json::json!({
            "server":name, "description":config.description, "capabilities":config.capabilities,
            "enabled":config.enabled,"connected":self.clients.contains_key(name),"transport":transport_name(config)
        })).collect::<Vec<_>>())
    }

    /// Disconnects a lazily started server. The next use reconnects it.
    pub fn disconnect(&mut self, server: &str) -> bool {
        self.clients.remove(server).is_some()
    }

    /// Connects only the named server and discovers its tools.
    ///
    /// # Errors
    ///
    /// Returns an error when the server is unknown, disabled, or unavailable.
    pub async fn discover_tools(&mut self, server: &str) -> Result<Vec<McpTool>, McpError> {
        let _timer = tool::telemetry::Timer::new("mcp.discover");
        self.client(server).await?.list_tools().await
    }

    /// Calls one tool on one named server, connecting that server on demand.
    ///
    /// # Errors
    ///
    /// Returns an error when the server cannot connect or the tool call fails.
    pub async fn call_tool(
        &mut self,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<ToolCallResult, McpError> {
        let _timer = tool::telemetry::Timer::new("mcp.call");
        self.client(server).await?.call_tool(tool, arguments).await
    }

    async fn client(&mut self, server: &str) -> Result<&mut McpClient, McpError> {
        if !self.clients.contains_key(server) {
            let config = self
                .configs
                .get(server)
                .ok_or_else(|| McpError::UnknownServer(server.to_owned()))?;
            if !config.enabled {
                return Err(McpError::DisabledServer(server.to_owned()));
            }
            let _timer = tool::telemetry::Timer::new("mcp.connect");
            let client = McpClient::connect(server, config).await?;
            self.clients.insert(server.to_owned(), client);
        }
        self.clients
            .get_mut(server)
            .ok_or_else(|| McpError::UnknownServer(server.to_owned()))
    }
}

fn transport_name(config: &ServerConfig) -> &'static str {
    match config.transport {
        crate::TransportConfig::Stdio { .. } => "stdio",
        crate::TransportConfig::Http { .. } => "http",
        crate::TransportConfig::WebSocket { .. } => "websocket",
    }
}
