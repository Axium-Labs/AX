//! All built-in local execution enters one worker boundary, including reads.
use crate::{Capability, ResourceAccess, RunContext, SafetyLevel, Tool, ToolError, ToolOutput};
use async_trait::async_trait;
use sandbox::{CommandSpec, SandboxManager, SandboxMode};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct SandboxedTool {
    inner: Arc<dyn Tool>,
    root: PathBuf,
    child_state: Option<PathBuf>,
    manager: OnceLock<Result<SandboxManager, String>>,
}
impl SandboxedTool {
    pub fn new(inner: Arc<dyn Tool>, root: PathBuf) -> Self {
        Self {
            inner,
            root,
            child_state: None,
            manager: OnceLock::new(),
        }
    }
    fn manager(&self) -> Result<&SandboxManager, ToolError> {
        self.manager
            .get_or_init(|| {
                match &self.child_state {
                    Some(state) => SandboxManager::for_child_workspace(self.root.clone(), state),
                    None => SandboxManager::for_workspace(self.root.clone()),
                }
                .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| sandbox::SandboxViolation(e.clone()).into())
    }
}
#[async_trait]
impl Tool for SandboxedTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::Sandboxed
    }
    fn fork_for_run(&self, context: &RunContext) -> Option<Arc<dyn Tool>> {
        // Preserve the boundary instead of delegating to the raw child tool.
        let mut tool = Self::new(self.inner.clone(), context.cwd.clone());
        tool.child_state = Some(context.state_dir.clone());
        Some(Arc::new(tool))
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        // Approval is reduced only by the deterministic fact that this tool is
        // bound to a prepared confined policy. The process-wide configured mode
        // is not that fact: it can disagree with the policy that will actually
        // run the call. An unprepared or unconfined binding keeps the tool's own
        // safety level, so a missing backend can never lower approval.
        let confined = self
            .manager()
            .is_ok_and(|manager| manager.policy().mode != SandboxMode::Off);
        if confined {
            SafetyLevel::Safe
        } else {
            self.inner.safety(input)
        }
    }
    fn capability(&self, input: &Value) -> Capability {
        self.inner.capability(input)
    }
    fn recursive_search(&self) -> bool {
        self.inner.recursive_search()
    }
    fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
        let mut input = input.clone();
        if let Some(path) = input["path"].as_str() {
            input["path"] = Value::String(self.root.join(path).to_string_lossy().into());
        }
        self.inner.resources(&input)
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        match self.execute_output(input).await? {
            ToolOutput::Text(text) => Ok(text),
            ToolOutput::Image { .. } => {
                Err(ToolError::Execution("multimodal output required".into()))
            }
        }
    }
    async fn execute_output(&self, input: Value) -> Result<ToolOutput, ToolError> {
        let manager = self.manager()?;
        if manager.policy().mode == SandboxMode::Off {
            return self.inner.execute_output(input).await;
        }
        let mut spec = CommandSpec::new(std::env::current_exe()?);
        spec.args = vec!["--ax-sandbox-worker".into(), self.name().into()];
        spec.stdin = std::process::Stdio::piped();
        let mut child = manager.spawn(spec)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::Execution("worker stdin unavailable".into()))?;
        let payload =
            serde_json::to_vec(&input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        // Read output concurrently, preventing pipe-capacity deadlocks.
        let writer = async move {
            stdin.write_all(&payload).await?;
            stdin.shutdown().await
        };
        let (written, output) = tokio::join!(writer, child.wait_with_output());
        written?;
        let output = output?;
        if !output.status.success() {
            return Err(sandbox::SandboxViolation(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
            .into());
        }
        let result: Result<ToolOutput, String> = serde_json::from_slice(&output.stdout)
            .map_err(|e| ToolError::Execution(format!("invalid sandbox worker result: {e}")))?;
        result.map_err(|e| {
            if e.contains("Read-only file system")
                || e.contains("Permission denied")
                || e.contains("Operation not permitted")
                || e.contains("No such file or directory")
            {
                sandbox::SandboxViolation(e).into()
            } else {
                ToolError::Execution(e)
            }
        })
    }
}

pub(crate) struct UnboundTool(pub Arc<dyn Tool>);
#[async_trait]
impl Tool for UnboundTool {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn description(&self) -> &str {
        self.0.description()
    }
    fn input_schema(&self) -> Value {
        self.0.input_schema()
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        self.0.safety(input)
    }
    fn capability(&self, input: &Value) -> Capability {
        self.0.capability(input)
    }
    async fn execute(&self, _: Value) -> Result<String, ToolError> {
        Err(sandbox::SandboxViolation(format!(
            "tool {} has no sandbox execution binding",
            self.name()
        ))
        .into())
    }
}

/// Called before CLI/runtime initialization. No credentials or providers load.
///
/// # Errors
///
/// Returns an error when the process is not the confined worker, when the
/// request cannot be read, when the tool name is unknown, or when the tool
/// itself fails.
pub async fn sandbox_worker(name: &str) -> Result<(), ToolError> {
    sandbox::verify_worker()?;
    let mut bytes = Vec::new();
    tokio::io::stdin()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .await?;
    let input: Value =
        serde_json::from_slice(&bytes).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    if let Some(path) = input["path"].as_str() {
        sandbox::authorize_workspace_path(
            std::path::Path::new(path),
            input["operation"] == "write",
        )?;
    }
    let tool: Box<dyn Tool> = match name {
        "shell" => Box::new(crate::ShellTool),
        "filesystem" => Box::new(crate::FilesystemTool),
        "patch" => Box::new(crate::PatchTool),
        "search" => Box::new(crate::SearchTool),
        "view_image" => Box::new(crate::ViewImageTool::new(std::env::current_dir()?)),
        _ => return Err(ToolError::Unknown(name.into())),
    };
    let result = tool.execute_output(input).await.map_err(|e| e.to_string());
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|e| ToolError::Execution(e.to_string()))?
    );
    Ok(())
}
