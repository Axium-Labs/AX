//! Child lifecycle adapters; execution stays in AgentKernel/AgentSupervisor.
use crate::{AgentError, AgentEvent, AgentKernel, AgentSupervisor};
use async_trait::async_trait;
use model::Message;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChildOutcome {
    pub success: bool,
    pub output: String,
}

/// Raw history and terminal receipts live in the child's own session.
pub trait ChildCheckpoint: Send {
    /// # Errors
    /// Returns errors writing the child's own raw history.
    fn save(&mut self, messages: &[Message]) -> Result<(), AgentError>;
    /// # Errors
    /// Returns errors writing the child's terminal receipt.
    fn finish(&mut self, outcome: &ChildOutcome) -> Result<(), AgentError>;
}

pub struct PreparedChild {
    pub run: ChildRun,
    pub kernel: AgentKernel,
    pub checkpoint: Box<dyn ChildCheckpoint>,
    pub terminal: Option<ChildOutcome>,
}

/// The composition root owns workspace/session/memory provisioning.
#[async_trait]
pub trait ChildHost: Send + Sync {
    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        previous: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError>;
}

/// Recover a final receipt from durable history, including unresolved tool errors.
#[must_use]
pub fn terminal_outcome(messages: &[Message]) -> Option<ChildOutcome> {
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
    Some(ChildOutcome {
        success: failures.is_empty(),
        output: if failures.is_empty() {
            last.content.clone()
        } else {
            format!(
                "{}\nUnresolved tool failures: {}",
                last.content,
                failures.join("\n")
            )
        },
    })
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

    /// Forks only explicit input/history and tools rebound to this child's scope.
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

    #[allow(clippy::too_many_lines)] // Child execution and durable receipt must remain one transaction.
    pub(crate) async fn execute_next_child<F, H>(
        &mut self,
        emit: &std::sync::Mutex<F>,
        checkpoint: &mut H,
    ) -> Result<(), AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let task = self
            .task_queue
            .as_ref()
            .and_then(|queue| {
                queue
                    .tasks
                    .iter()
                    .find(|task| task.status == crate::task_queue::TaskStatus::Running)
            })
            .expect("active child task");
        let input = task.title.clone();
        self.execution
            .lock()
            .unwrap()
            .current_step
            .clone_from(&input);
        let previous = task.child.clone();
        let host = self.child_host.as_ref().expect("child host").clone();
        let outcome = match host.prepare(self, &input, previous.as_ref()).await {
            Ok(mut child) => {
                self.task_queue
                    .as_mut()
                    .unwrap()
                    .current_mut()
                    .unwrap()
                    .child = Some(child.run.clone());
                // Persist child identity BEFORE executing its first model/tool call.
                self.checkpoint_queue(checkpoint)?;
                let event_session = child.run.session_id.clone();
                if let Some(outcome) = child.terminal {
                    outcome
                } else {
                    let result = AgentSupervisor::run_child(
                        &mut child.kernel,
                        &input,
                        Box::new(|mut event| {
                            match &mut event {
                                AgentEvent::ToolStarted { id, .. }
                                | AgentEvent::ToolFinished { id, .. } => {
                                    *id = format!("{event_session}:{id}");
                                }
                                _ => {}
                            }
                            // Child text/turn boundaries are not controller final output.
                            if !matches!(
                                event,
                                AgentEvent::ContentDelta { .. }
                                    | AgentEvent::TurnStarted
                                    | AgentEvent::TurnFinished
                            ) {
                                (emit.lock().unwrap())(event);
                            }
                        }),
                        Box::new(|messages| {
                            child.checkpoint.save(messages)?;
                            if let Some(state) = messages
                                .iter()
                                .rev()
                                .filter(|m| m.role == model::Role::System)
                                .find_map(|m| {
                                    m.content
                                        .strip_prefix(crate::execution::STATE_PREFIX)
                                        .and_then(|json| {
                                            serde_json::from_str::<crate::ExecutionState>(json).ok()
                                        })
                                })
                            {
                                let changed = self
                                    .execution
                                    .lock()
                                    .unwrap()
                                    .absorb_child(&event_session, &state);
                                if changed {
                                    self.raw_turn_messages
                                        .push(self.execution_state().snapshot());
                                    checkpoint(&self.raw_turn_messages)?;
                                }
                            }
                            Ok(())
                        }),
                    )
                    .await;
                    let mut outcome = match result {
                        Ok(output) => ChildOutcome {
                            success: true,
                            output,
                        },
                        Err(error) => ChildOutcome {
                            success: false,
                            output: error.to_string(),
                        },
                    };
                    // A receipt closes the crash gap before the controller advances.
                    if let Err(error) = child.checkpoint.finish(&outcome) {
                        outcome.success = false;
                        outcome.output =
                            format!("{}\nChild receipt failed: {error}", outcome.output);
                    }
                    outcome
                }
            }
            Err(error) => ChildOutcome {
                success: false,
                output: error.to_string(),
            },
        };
        let queue = self.task_queue.as_mut().unwrap();
        let task = queue.current_mut().unwrap();
        task.status = if outcome.success {
            crate::task_queue::TaskStatus::Completed
        } else {
            crate::task_queue::TaskStatus::Failed
        };
        task.failure_reason = (!outcome.success).then(|| outcome.output.clone());
        task.outcome = Some(outcome.output);
        queue.advance();
        self.checkpoint_queue(checkpoint)
    }
}

pub type ChildSaveSink<'a> = Box<dyn FnMut(&[Message]) -> Result<(), AgentError> + Send + 'a>;

impl AgentSupervisor {
    /// Run one durable child through the existing model/tool loop.
    pub fn run_child<'a>(
        kernel: &'a mut AgentKernel,
        input: &'a str,
        emit: Box<dyn FnMut(AgentEvent) + Send + 'a>,
        checkpoint: ChildSaveSink<'a>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, AgentError>> + Send + 'a>>
    {
        Box::pin(async move { kernel.run_turn_checkpointed(input, emit, checkpoint).await })
    }
}
