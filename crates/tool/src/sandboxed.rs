//! All built-in local execution enters one worker boundary, including reads.
#![forbid(unsafe_code)]
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
    cwd: PathBuf,
    child_state: Option<PathBuf>,
    manager: OnceLock<SandboxManager>,
}
impl SandboxedTool {
    pub fn new(inner: Arc<dyn Tool>, root: PathBuf) -> Self {
        Self {
            inner,
            cwd: root.clone(),
            root,
            child_state: None,
            manager: OnceLock::new(),
        }
    }
    fn manager(&self) -> Result<&SandboxManager, ToolError> {
        if let Some(manager) = self.manager.get() {
            return Ok(manager);
        }
        let manager = match &self.child_state {
            Some(state) => SandboxManager::for_child_workspace(self.root.clone(), state),
            None => SandboxManager::for_workspace(self.root.clone()),
        }?;
        // Only success is cached; preparation errors can be retried next call.
        let _ = self.manager.set(manager);
        Ok(self.manager.get().expect("prepared manager"))
    }
}
#[async_trait]
impl Tool for SandboxedTool {
    fn execution_usage_incomplete(&self, input: &Value) -> bool {
        self.inner.execution_usage_incomplete(input)
    }
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::Sandboxed
    }
    fn fork_for_run(&self, context: &RunContext) -> Option<Arc<dyn Tool>> {
        // Rebind the wrapped tool to the child scope first (workspace path
        // binding lives there), then re-apply the boundary so a child keeps the
        // same confinement. Tools that carry no run state are reused as-is.
        let inner = self
            .inner
            .fork_for_run(context)
            .unwrap_or_else(|| Arc::clone(&self.inner));
        let mut tool = Self::new(inner, context.workspace_root.clone());
        tool.cwd.clone_from(&context.cwd);
        tool.child_state = Some(context.state_dir.clone());
        Some(Arc::new(tool))
    }
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn guidance(&self) -> Option<&'static str> {
        // Confinement wraps a capability; it never changes how the capability is
        // used, so guidance passes through unchanged.
        self.inner.guidance()
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
            .manager
            .get()
            .cloned()
            .or_else(|| SandboxManager::prepared_for_workspace(&self.root))
            .is_some_and(|manager| manager.policy().mode != SandboxMode::Off);
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
        for field in ["path", "root"] {
            if let Some(path) = input[field].as_str() {
                input[field] = Value::String(self.cwd.join(path).to_string_lossy().into());
            }
        }
        self.inner.resources(&input)
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        match self.execute_output(input).await? {
            ToolOutput::Text(text) | ToolOutput::Verbatim { text, .. } => Ok(text),
            ToolOutput::Image { .. } => {
                Err(ToolError::Execution("multimodal output required".into()))
            }
        }
    }
    async fn execute_output(&self, input: Value) -> Result<ToolOutput, ToolError> {
        if self.manager.get().is_none() {
            let root = self.root.clone();
            let state = self.child_state.clone();
            let manager = tokio::task::spawn_blocking(move || match state {
                Some(state) => SandboxManager::for_child_workspace(root, &state),
                None => SandboxManager::for_workspace(root),
            })
            .await
            .map_err(|error| {
                sandbox::SandboxViolation(format!("worker preparation failed: {error}"))
            })??;
            let _ = self.manager.set(manager);
        }
        let manager = self.manager()?;
        if manager.policy().mode == SandboxMode::Off {
            return self.inner.execute_output(input).await;
        }
        let mut spec = CommandSpec::new(std::env::current_exe()?);
        spec.args = vec!["--ax-sandbox-worker".into(), self.name().into()];
        spec.cwd = Some(self.cwd.clone());
        spec.stdin = std::process::Stdio::piped();
        let mut child = manager.spawn(spec)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| sandbox::SandboxViolation("worker stdin unavailable".into()))?;
        let payload =
            serde_json::to_vec(&input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        // Read output concurrently, preventing pipe-capacity deadlocks.
        let writer = async move {
            stdin.write_all(&payload).await?;
            stdin.shutdown().await
        };
        let (written, output) = tokio::join!(writer, child.wait_with_output());
        written.map_err(sandbox::SandboxViolation::from)?;
        let output = output.map_err(sandbox::SandboxViolation::from)?;
        if !output.status.success() {
            return Err(sandbox::SandboxViolation(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )
            .into());
        }
        let result: WorkerResult = serde_json::from_slice(&output.stdout).map_err(|e| {
            sandbox::SandboxViolation(format!("invalid sandbox worker result: {e}"))
        })?;
        match result {
            WorkerResult::Output(output) => Ok(output),
            WorkerResult::ToolError(error) => Err(ToolError::Execution(error)),
            WorkerResult::ConfinementError(error) => Err(sandbox::SandboxViolation(error).into()),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum WorkerResult {
    Output(ToolOutput),
    ToolError(String),
    ConfinementError(String),
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
    let result = match worker_request(name).await {
        Ok(output) => WorkerResult::Output(output),
        Err(ToolError::SandboxViolation(error)) => {
            WorkerResult::ConfinementError(error.to_string())
        }
        Err(error) => WorkerResult::ToolError(error.to_string()),
    };
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|e| ToolError::Execution(e.to_string()))?
    );
    Ok(())
}
async fn worker_request(name: &str) -> Result<ToolOutput, ToolError> {
    let mut bytes = Vec::new();
    tokio::io::stdin()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .await?;
    let input: Value =
        serde_json::from_slice(&bytes).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    for field in ["path", "root"] {
        if let Some(path) = input[field].as_str() {
            sandbox::authorize_workspace_path(
                std::path::Path::new(path),
                input["operation"] == "write",
            )?;
        }
    }
    let workspace = std::env::current_dir()?;
    let tool: Box<dyn Tool> = match name {
        "task_source" => Box::new(crate::TaskSourceTool::new(workspace.clone())),
        "shell" => Box::new(crate::ShellTool),
        "filesystem" => Box::new(crate::FilesystemTool),
        "patch" => Box::new(crate::PatchTool),
        "search" => Box::new(crate::SearchTool::new(workspace.clone())),
        "find_files" => Box::new(crate::FindFilesTool::new(workspace.clone())),
        "glob" => Box::new(crate::FindFilesTool::glob(workspace.clone())),
        "view_image" => Box::new(crate::ViewImageTool::new(workspace)),
        _ => return Err(ToolError::Unknown(name.into())),
    };
    tool.execute_output(input).await
}
