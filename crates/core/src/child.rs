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
    pub memory_scope: String,
    /// Durable store outside the disposable workspace; None for legacy runs.
    #[serde(default)]
    pub state_dir: Option<PathBuf>,
    #[serde(default)]
    pub execution_budget: Option<crate::ExecutionBudget>,
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

/// Recover a final receipt from durable history, including unresolved tool errors.
#[must_use]
pub fn terminal_result(messages: &[Message]) -> Option<ChildResult> {
    let last = messages
        .iter()
        .rev()
        .find(|m| m.role != model::Role::System)?;
    if last.role != model::Role::Assistant || !last.tool_calls.is_empty() {
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
        self.child_concurrency = concurrency.clamp(1, 64);
        self
    }

    #[must_use]
    pub fn child_concurrency(&self) -> usize {
        self.child_concurrency
    }

    /// Forks only explicit input/history and tools rebound to this child's scope.
    ///
    /// Same registry, different binding: a child never gets a second
    /// implementation of search, filesystem, patch, shell or permissions.
    #[must_use]
    pub fn fork_child(&self, run: ChildRun, input: &str, messages: Vec<Message>) -> Self {
        let context = tool::RunContext {
            cwd: run.cwd.clone(),
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
                && let Ok(mut store) = kernel.result_reader.0.write()
            {
                store.insert(id.clone(), result.raw_output);
            }
        }
        kernel.child_run = Some(run);
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
