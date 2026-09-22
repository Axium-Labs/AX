//! Tool contracts, registry, and lightweight built-in tools.

mod filesystem;
mod shell;

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

pub use filesystem::FilesystemTool;
pub use shell::ShellTool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyLevel {
    Safe,
    RequiresApproval,
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    Unknown(String),
    #[error("invalid tool input: {0}")]
    InvalidInput(String),
    #[error("tool execution failed: {0}")]
    Execution(String),
    #[error("permission denied for tool: {0}")]
    PermissionDenied(String),
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn safety(&self, input: &Value) -> SafetyLevel;
    async fn execute(&self, input: Value) -> Result<String, ToolError>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.name().to_owned(), Arc::new(tool));
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.values()
    }

    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<_> = self.tools.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_returns_sorted_names() {
        let mut registry = ToolRegistry::new();
        registry.register(ShellTool);
        registry.register(FilesystemTool);
        assert_eq!(registry.names(), vec!["filesystem", "shell"]);
    }

    #[test]
    fn shell_always_requires_approval() {
        assert_eq!(
            ShellTool.safety(&serde_json::json!({ "command": "pwd" })),
            SafetyLevel::RequiresApproval
        );
    }
}
