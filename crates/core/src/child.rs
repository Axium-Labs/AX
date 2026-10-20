//! Child lifecycle adapters; execution stays in AgentKernel/AgentSupervisor.
//!
//! The controller never learns a second execution path. A child is the *same*
//! kernel with a different context, cwd, memory scope, sandbox boundary and
//! task input; the tool registry is rebound to the child's run context rather
//! than replaced. Dispatching several children at once is
//! [`crate::child_dispatch`]'s job.
use crate::{AgentError, AgentKernel};
use async_trait::async_trait;
use model::Message;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::child_result::{ChildResult, ChildStatus};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildRun {
    pub goal_id: String,
    pub session_id: String,
    pub cwd: PathBuf,
    /// Lifecycle/sandbox root when execution starts in a repository subdirectory.
    #[serde(default)]
    pub workspace_root: Option<PathBuf>,
    pub memory_scope: String,
    /// Durable store outside the disposable workspace; None for legacy runs.
    #[serde(default)]
    pub state_dir: Option<PathBuf>,
    #[serde(default)]
    pub execution_budget: Option<crate::ExecutionBudget>,
}

impl ChildRun {
    #[must_use]
    pub fn workspace_root(&self) -> &std::path::Path {
        self.workspace_root.as_deref().unwrap_or(&self.cwd)
    }
}

/// Raw history and terminal receipts live in the child's own session.
pub trait ChildCheckpoint: Send {
    /// # Errors
    /// Returns errors writing the child's own raw history.
    fn save(&mut self, messages: &[Message]) -> Result<(), AgentError>;
    /// # Errors
    /// Returns errors writing the child's terminal receipt.
    ///
    /// The host may enrich `result` (diff statistics, authoritative changed
    /// files) before it is written; the controller only sees what lands here.
    fn finish(&mut self, result: &mut ChildResult) -> Result<(), AgentError>;
}

pub struct PreparedChild {
    pub run: ChildRun,
    pub kernel: AgentKernel,
    pub checkpoint: Box<dyn ChildCheckpoint>,
    /// A receipt recovered from durable history: the child already finished in
    /// an earlier process, so it must never be re-run.
    pub terminal: Option<ChildResult>,
}

/// The composition root owns workspace/session/memory provisioning.
#[async_trait]
pub trait ChildHost: Send + Sync {
    /// Rebind lazy child provisioning to the delegated child's workspace.
    fn fork_for_child(&self, _run: &ChildRun) -> Option<std::sync::Arc<dyn ChildHost>> {
        None
    }

    /// Persist a task-local setup failure even when no child session could be
    /// created. A missing workspace must not make this receipt overwrite a sibling.
    /// # Errors
    /// Returns an error when the host cannot durably save the failure receipt.
    fn persist_preparation_failure(
        &self,
        _task: &crate::task_queue::QueuedTask,
        _result: &mut ChildResult,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    async fn prepare_task(
        &self,
        controller: &AgentKernel,
        task: &crate::task_queue::QueuedTask,
    ) -> Result<PreparedChild, AgentError> {
        if task.workspace != crate::task_queue::WorkspaceSpec::default()
            || task.output_dir.is_some()
        {
            return Err(tool::ToolError::InvalidInput(
                "host does not support task workspace/output specification".into(),
            )
            .into());
        }
        self.prepare(controller, task.task_input(), task.child.as_ref())
            .await
    }

    async fn prepare_with_policy(
        &self,
        controller: &AgentKernel,
        input: &str,
        policy: &crate::ChildPolicy,
    ) -> Result<PreparedChild, AgentError> {
        if policy.workspace != crate::child_policy::WorkspaceInheritance::Isolated {
            return Err(tool::ToolError::InvalidInput(
                "host does not support requested workspace policy".into(),
            )
            .into());
        }
        self.prepare(controller, input, None).await
    }

    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        previous: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError>;
}

// Read-only compatibility with pre-continuation harness checkpoints.
pub(crate) const LEGACY_COMPLETION_CHECK: &str = "completion_check";
pub(crate) const LEGACY_COMPLETION_PENDING: &str = "[ax-completion-pending]";

/// Recover a final receipt from durable history, including unresolved tool errors.
#[must_use]
pub fn terminal_result(messages: &[Message]) -> Option<ChildResult> {
    terminal_result_reviewed(messages, false)
}

fn terminal_result_reviewed(messages: &[Message], reviewed: bool) -> Option<ChildResult> {
    let last = messages
        .iter()
        .rev()
        .find(|m| m.role != model::Role::System)?;
    // Completion review acknowledgements are control history, not a replacement
    // for the candidate final or evidence of repair. Recover the original final
    // if a process stopped after accepting review but before saving the receipt.
    if last.role == model::Role::Tool
        && let Some(index) = messages.iter().rposition(|message| {
            message.tool_calls.len() == 1
                && message.tool_calls[0].id == last.tool_call_id.as_deref().unwrap_or_default()
                && message.tool_calls[0].function.name == LEGACY_COMPLETION_CHECK
                && serde_json::from_str::<serde_json::Value>(
                    &message.tool_calls[0].function.arguments,
                )
                .is_ok_and(|value| value["state"] == "complete")
        })
    {
        return terminal_result_reviewed(&messages[..index], true);
    }
    if last.role != model::Role::Assistant || !last.tool_calls.is_empty() {
        return None;
    }
    let final_index = messages
        .iter()
        .rposition(|m| m.role != model::Role::System)?;
    let preceding = final_index.checked_sub(1).and_then(|i| messages.get(i));
    if preceding.is_some_and(|m| {
        m.content == "[ax-model-continuation]" || m.content == crate::harness::HANDOFF_PENDING
    }) {
        return None;
    }
    if preceding.is_some_and(|m| {
        matches!(
            m.content.as_str(),
            "[ax-goal-pending]" | "[ax-stop-guard-pending]"
        )
    }) && !messages[final_index + 1..].iter().any(|m| {
        matches!(
            m.content.as_str(),
            "[ax-goal-allowed]" | "[ax-stop-guard-allowed]"
        )
    }) {
        return None;
    }
    if !reviewed && preceding.is_some_and(|m| m.content.starts_with(LEGACY_COMPLETION_PENDING)) {
        return None;
    }
    let round = messages.iter().rposition(|m| !m.tool_calls.is_empty());
    let failures = round
        .map(|start| {
            messages[start + 1..]
                .iter()
                .filter(|m| m.role == model::Role::Tool)
                .filter_map(|m| {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&m.content) {
                        (value["status"] == "error").then(|| {
                            value["raw_output"]
                                .as_str()
                                .or_else(|| value["output"].as_str())
                                .or_else(|| value["summary"].as_str())
                                .unwrap_or("child tool failed")
                                .to_owned()
                        })
                    } else {
                        Some(m.content.clone())
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut result = ChildResult::new(String::new(), String::new(), ChildStatus::Completed);
    result.summary.clone_from(&last.content);
    result.diagnostics.clone_from(&failures);
    if failures.is_empty() {
        return Some(result);
    }
    let reason = format!(
        "{}\nUnresolved tool failures: {}",
        last.content,
        failures.join("\n")
    );
    let mut failed = ChildResult::failed(String::new(), String::new(), ChildStatus::Failed, reason);
    failed.summary.clone_from(&last.content);
    failed.diagnostics = failures;
    Some(failed)
}

impl AgentKernel {
    /// Apply the same parent ceiling and turn/tree budget to every host result.
    /// Hosts provision isolation; they cannot create a fresh delegation allowance.
    pub(crate) fn bind_child_delegation(&self, child: &mut PreparedChild) {
        child.kernel.approval = std::sync::Arc::clone(&self.approval);
        let host_profiles = std::mem::take(&mut child.kernel.permission_profiles);
        child
            .kernel
            .permission_profiles
            .clone_from(&self.permission_profiles);
        // Standard hosts already fork the parent's profiles. Preserve extra
        // host restrictions without doubling the inherited list at every level.
        if serde_json::to_value(&host_profiles).ok()
            != serde_json::to_value(&self.permission_profiles).ok()
        {
            child.kernel.permission_profiles.extend(host_profiles);
        }
        if child.kernel.permission_profiles.is_empty() {
            child
                .kernel
                .permission_profiles
                .push(tool::PermissionProfile::default());
        }
        child.kernel.subagent_config = self.subagent_config;
        child.kernel.subagent_depth = self.subagent_depth.saturating_add(1);
        if child.kernel.child_run.is_none() {
            child.kernel.child_run = Some(child.run.clone());
        }
        child.kernel.subagent_pool.clone_from(&self.subagent_pool);
        child.kernel.subagent_tools.clone_from(&self.subagent_tools);
        child
            .kernel
            .agent_templates
            .clone_from(&self.agent_templates);
        child.kernel.child_models.clone_from(&self.child_models);
        child.kernel.child_host = self.child_host.as_ref().map(|host| {
            host.fork_for_child(child.kernel.child_run.as_ref().unwrap_or(&child.run))
                .unwrap_or_else(|| std::sync::Arc::clone(host))
        });
        // Never keep a delegation tool that is still bound to another kernel.
        child.kernel.tools.remove("subagent");
        child.kernel.tools.remove("subagent_fork");
        child.kernel.tools.remove("spawn_agent");
    }

    #[must_use]
    pub fn with_child_host(mut self, host: std::sync::Arc<dyn ChildHost>) -> Self {
        self.child_host = Some(host);
        self
    }

    #[must_use]
    pub fn with_child_execution_budget(mut self, budget: crate::ExecutionBudget) -> Self {
        self.child_budget = Some(budget);
        self
    }

    #[must_use]
    pub fn child_execution_budget(&self) -> crate::ExecutionBudget {
        self.child_budget.unwrap_or(self.budget)
    }

    /// Bounded concurrency for independently ready children.
    #[must_use]
    pub fn with_child_concurrency(mut self, concurrency: usize) -> Self {
        self.subagent_config.max_concurrent = concurrency.clamp(1, 64);
        self
    }

    #[must_use]
    pub fn child_concurrency(&self) -> usize {
        self.subagent_config.max_concurrent.clamp(1, 64)
    }

    /// Forks only explicit input/history and tools rebound to this child's scope.
    ///
    /// Same registry, different binding: a child never gets a second
    /// implementation of search, filesystem, patch, shell or permissions.
    #[must_use]
    pub fn fork_child(&self, run: ChildRun, input: &str, messages: Vec<Message>) -> Self {
        let context = tool::RunContext {
            cwd: run.cwd.clone(),
            workspace_root: run.workspace_root().to_path_buf(),
            state_dir: run.state_dir.clone().unwrap_or_else(|| run.cwd.join(".ax")),
            session_id: run.session_id.clone(),
            memory_scope: run.memory_scope.clone(),
            input: input.to_owned(),
        };
        let mut kernel = self.fork_with_messages(messages);
        kernel.tools = self.tools.fork_for_run(&context);
        kernel.tools.register(kernel.result_reader.clone());
        kernel.budget = run
            .execution_budget
            .unwrap_or_else(|| self.child_execution_budget());
        for message in &kernel.messages {
            if let Some(id) = &message.tool_call_id
                && let Ok(result) = serde_json::from_str::<tool::ToolResult>(&message.content)
            {
                kernel.result_reader.restore(id, &result);
            }
        }
        kernel.execution_root = Some(run.workspace_root().to_path_buf());
        kernel.child_run = Some(run);
        kernel.subagent_depth = self.subagent_depth.saturating_add(1);
        kernel.child_host = None;
        kernel
    }
}

pub type ChildSaveSink<'a> = Box<dyn FnMut(&[Message]) -> Result<(), AgentError> + Send + 'a>;

impl crate::AgentSupervisor {
    /// Run one durable child through the existing model/tool loop.
    pub fn run_child<'a>(
        kernel: &'a mut AgentKernel,
        input: &'a str,
        emit: Box<dyn FnMut(crate::AgentEvent) + Send + 'a>,
        checkpoint: ChildSaveSink<'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, AgentError>> + Send + 'a>>
    {
        Box::pin(async move { kernel.run_turn_checkpointed(input, emit, checkpoint).await })
    }
}

#[cfg(test)]
#[path = "child_tests.rs"]
mod tests;
