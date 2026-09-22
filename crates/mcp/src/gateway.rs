//! Always-visible MCP capability gateway, with no transport opened for catalog reads.
use crate::McpManager;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
use tool::{Capability, SafetyLevel, Tool, ToolError};

pub struct McpGateway {
    manager: Arc<Mutex<McpManager>>,
}
impl McpGateway {
    pub fn new(manager: Arc<Mutex<McpManager>>) -> Self {
        Self { manager }
    }
}
#[async_trait]
impl Tool for McpGateway {
    fn name(&self) -> &'static str {
        "mcp"
    }
    fn description(&self) -> &'static str {
        "Discover configured external capabilities, including sleeping servers. Use catalog (no connection), then list_tools for a server, then call with its tool name and arguments. Do not invent tool names."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"action":{"enum":["catalog","list_tools","call"]},"server":{"type":"string"},"tool":{"type":"string"},"arguments":{"type":"object"}},"required":["action"],"additionalProperties":false})
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::Mcp
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        if input["action"] == "catalog" {
            SafetyLevel::Safe
        } else {
            SafetyLevel::RequiresApproval
        }
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let mut manager = self.manager.lock().await;
        if input["action"] == "catalog" {
            return Ok(manager.capability_catalog().to_string());
        }
        let server = input["server"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("server required".into()))?;
        let result = match input["action"].as_str() {
            Some("list_tools") => serde_json::to_value(
                manager
                    .discover_tools(server)
                    .await
                    .map_err(|e| ToolError::Execution(e.to_string()))?,
            )
            .map_err(|e| ToolError::Execution(e.to_string()))?,
            Some("call") => {
                let name = input["tool"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidInput("tool required".into()))?;
                let arguments = input.get("arguments").cloned().unwrap_or_else(|| json!({}));
                serde_json::to_value(
                    manager
                        .call_tool(server, name, arguments)
                        .await
                        .map_err(|e| ToolError::Execution(e.to_string()))?,
                )
                .map_err(|e| ToolError::Execution(e.to_string()))?
            }
            _ => return Err(ToolError::InvalidInput("invalid MCP action".into())),
        };
        Ok(result.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn sleeping_capabilities_are_visible_without_starting_server() {
        let config = toml::from_str::<crate::McpConfig>(
            r#"
            [servers.fake]
            transport="stdio"
            command="this-command-does-not-exist-ax-test"
            description="Project issue tracker"
            capabilities=["issues", "tasks"]
        "#,
        )
        .unwrap();
        let manager = Arc::new(Mutex::new(McpManager::new(config)));
        let gateway = McpGateway::new(manager.clone());
        let result = gateway.execute(json!({"action":"catalog"})).await.unwrap();
        assert!(result.contains("issues"));
        assert!(result.contains("Project issue tracker"));
        assert_eq!(manager.lock().await.connected_server_count(), 0);
        assert_eq!(
            gateway.permission(&json!({"action":"call"})).capability,
            Capability::Mcp
        );
    }
}
