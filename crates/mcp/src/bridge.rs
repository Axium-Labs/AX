use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;
use tool::{SafetyLevel, Tool, ToolError};

use crate::{McpError, McpManager};

#[derive(Clone)]
pub struct McpToolProxy {
    registered_name: String,
    description: String,
    input_schema: Value,
    read_only: bool,
    server: String,
    remote_name: String,
    manager: Arc<Mutex<McpManager>>,
}

impl McpToolProxy {
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    #[must_use]
    pub fn remote_name(&self) -> &str {
        &self.remote_name
    }
}

/// Connects one named server, discovers its tools, and creates registry proxies.
/// No other configured server is connected.
///
/// # Errors
///
/// Returns an error when the selected server cannot connect or list tools.
pub async fn discover_tool_proxies(
    manager: Arc<Mutex<McpManager>>,
    server: &str,
) -> Result<Vec<McpToolProxy>, McpError> {
    let remote_tools = manager.lock().await.discover_tools(server).await?;
    Ok(remote_tools
        .into_iter()
        .map(|remote| {
            let read_only = remote
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.get("readOnlyHint"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            McpToolProxy {
                registered_name: format!("mcp__{server}__{}", remote.name),
                description: remote.description.unwrap_or_else(|| {
                    format!("MCP tool '{}' from server '{server}'", remote.name)
                }),
                input_schema: remote.input_schema,
                read_only,
                server: server.to_owned(),
                remote_name: remote.name,
                manager: Arc::clone(&manager),
            }
        })
        .collect())
}

#[async_trait]
impl Tool for McpToolProxy {
    fn name(&self) -> &str {
        &self.registered_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn safety(&self, _input: &Value) -> SafetyLevel {
        if self.read_only {
            SafetyLevel::Safe
        } else {
            SafetyLevel::RequiresApproval
        }
    }

    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let result = self
            .manager
            .lock()
            .await
            .call_tool(&self.server, &self.remote_name, input)
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        serde_json::to_string(&result).map_err(|error| ToolError::Execution(error.to_string()))
    }
}
