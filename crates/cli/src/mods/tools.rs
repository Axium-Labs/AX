use super::ModHost;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tool::{
    Capability, ExecutionBoundary, PermissionProfile, ResourceAccess, RunContext, SafetyLevel,
    Tool, ToolError, ToolOutput,
};

pub(super) struct WrappedTool {
    pub host: ModHost,
    pub inner: Arc<dyn Tool>,
}
#[async_trait]
impl Tool for WrappedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn guidance(&self) -> Option<&'static str> {
        self.inner.guidance()
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        self.inner.safety(input)
    }
    fn capability(&self, input: &Value) -> Capability {
        self.inner.capability(input)
    }
    fn permission(&self, input: &Value) -> tool::ToolPermission {
        self.inner.permission(input)
    }
    // A Mod can issue additional effects, so its outer call holds the global lease.
    fn resources(&self, _input: &Value) -> Vec<ResourceAccess> {
        vec![ResourceAccess::exclusive()]
    }
    fn execution_boundary(&self) -> ExecutionBoundary {
        self.inner.execution_boundary()
    }
    fn runtime_owned_resources(&self) -> bool {
        self.inner.runtime_owned_resources()
    }
    fn inheritance_class(&self) -> tool::InheritanceClass {
        self.inner.inheritance_class()
    }
    // Children get their own rebound tools, never the parent's Node process/state.
    fn fork_for_run(&self, context: &RunContext) -> Option<Arc<dyn Tool>> {
        self.inner.fork_for_run(context)
    }
    fn fork_memory(&self, input: &str, readonly: bool) -> Option<Arc<dyn Tool>> {
        self.inner.fork_memory(input, readonly)
    }
    fn fork_skills(&self, selected: Option<&[String]>) -> Option<Arc<dyn Tool>> {
        self.inner.fork_skills(selected)
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        match self.execute_output_constrained(input, &[]).await? {
            ToolOutput::Text(text) => Ok(text),
            ToolOutput::Image { description, .. } => Ok(description),
        }
    }
    async fn execute_output_constrained(
        &self,
        input: Value,
        profiles: &[PermissionProfile],
    ) -> Result<ToolOutput, ToolError> {
        self.host
            .tool_call(self.name(), input, Some(self.inner.clone()), profiles)
            .await
    }
}
pub(crate) struct ModTool {
    host: ModHost,
    name: String,
    description: String,
    schema: Value,
}
impl ModTool {
    pub(super) fn new(host: ModHost, spec: &Value) -> Result<Self> {
        let name = spec["name"].as_str().context("Missing Mod tool name")?;
        Ok(Self {
            host,
            name: name.into(),
            description: spec["description"].as_str().unwrap_or_default().into(),
            schema: spec["inputSchema"].clone(),
        })
    }
}
#[async_trait]
impl Tool for ModTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> Value {
        self.schema.clone()
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::RequiresApproval
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::Mcp
    }
    // Node mods are trusted extensions, so OS confinement rejects their unbound tools.
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        match self.execute_output_constrained(input, &[]).await? {
            ToolOutput::Text(text) => Ok(text),
            ToolOutput::Image { description, .. } => Ok(description),
        }
    }
    async fn execute_output_constrained(
        &self,
        input: Value,
        profiles: &[PermissionProfile],
    ) -> Result<ToolOutput, ToolError> {
        self.host.tool_call(&self.name, input, None, profiles).await
    }
}
