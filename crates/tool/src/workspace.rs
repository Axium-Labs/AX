//! Resolve child tool paths without changing the process-wide cwd.
use crate::{Capability, ResourceAccess, SafetyLevel, Tool, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug)]
pub struct RunContext {
    pub cwd: PathBuf,
    pub state_dir: PathBuf,
    pub session_id: String,
    pub memory_scope: String,
    pub input: String,
}

pub struct WorkspaceTool {
    tool: Arc<dyn Tool>,
    cwd: PathBuf,
}
impl WorkspaceTool {
    #[must_use]
    pub fn new(tool: Arc<dyn Tool>, cwd: PathBuf) -> Self {
        Self { tool, cwd }
    }
    /// Binds a relative `path` argument to this workspace. Path binding only;
    /// object confinement belongs to Sandbox Manager.
    fn resolve(&self, mut input: Value) -> Value {
        if let Some(path) = input["path"].as_str() {
            let path = self.cwd.join(path);
            input["path"] = Value::String(path.to_string_lossy().into_owned());
        }
        input
    }
}
#[async_trait]
impl Tool for WorkspaceTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        self.tool.execution_boundary()
    }
    fn runtime_owned_resources(&self) -> bool {
        self.tool.runtime_owned_resources()
    }
    fn recursive_search(&self) -> bool {
        self.tool.recursive_search()
    }
    fn name(&self) -> &str {
        self.tool.name()
    }
    fn description(&self) -> &str {
        self.tool.description()
    }
    fn input_schema(&self) -> Value {
        self.tool.input_schema()
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        self.tool.safety(input)
    }
    fn capability(&self, input: &Value) -> Capability {
        self.tool.capability(input)
    }
    fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
        self.tool.resources(&self.resolve(input.clone()))
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        self.tool.execute(self.resolve(input)).await
    }
    async fn execute_output(&self, input: Value) -> Result<ToolOutput, ToolError> {
        self.tool.execute_output(self.resolve(input)).await
    }
}
