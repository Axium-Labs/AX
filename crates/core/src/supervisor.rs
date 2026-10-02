//! Bounded-concurrency supervision of independent agent contexts.
//!
//! A supervisor clones a template [`AgentKernel`] per task, so every task owns
//! its own message history while sharing the provider, tools and approval
//! policy.

use std::collections::VecDeque;

use model::Message;
use tokio::task::JoinSet;

use crate::{AgentError, AgentEvent, AgentKernel};

#[derive(Clone, Debug)]
pub struct AgentTask {
    pub id: String,
    pub prompt: String,
    pub context: Vec<Message>,
}

#[derive(Debug)]
pub struct AgentTaskResult {
    pub id: String,
    pub result: Result<String, AgentError>,
}

#[derive(Clone, Debug)]
pub enum MultiAgentEvent {
    Started {
        id: String,
    },
    /// Boxed: `AgentEvent` is much larger than the lifecycle variants, and this
    /// channel carries one event per streaming delta.
    Runtime {
        id: String,
        event: Box<AgentEvent>,
    },
    Finished {
        id: String,
    },
}

pub struct AgentSupervisor {
    pub(crate) template: AgentKernel,
    pub(crate) max_concurrency: usize,
}

impl AgentSupervisor {
    #[must_use]
    pub fn new(template: AgentKernel, max_concurrency: usize) -> Self {
        Self {
            template,
            max_concurrency: max_concurrency.max(1),
        }
    }

    /// Runs independent agent contexts with bounded concurrency.
    ///
    /// # Errors
    ///
    /// Individual model/tool failures are returned inside each task result.
    /// This method returns a top-level error only when a worker task panics or is cancelled.
    pub async fn run_tasks(
        &self,
        tasks: Vec<AgentTask>,
        events: Option<tokio::sync::mpsc::UnboundedSender<MultiAgentEvent>>,
    ) -> Result<Vec<AgentTaskResult>, AgentError> {
        let mut pending = tasks.into_iter().collect::<VecDeque<_>>();
        let mut running = JoinSet::new();
        let mut results = Vec::with_capacity(pending.len());

        while !pending.is_empty() || !running.is_empty() {
            while running.len() < self.max_concurrency
                && let Some(task) = pending.pop_front()
            {
                let mut kernel = self.template.fork_with_messages(task.context);
                let sender = events.clone();
                running.spawn(async move {
                    if let Some(sender) = &sender {
                        let _ = sender.send(MultiAgentEvent::Started {
                            id: task.id.clone(),
                        });
                    }
                    let id = task.id;
                    let event_id = id.clone();
                    let result = kernel
                        .run_turn(task.prompt, |event| {
                            if let Some(sender) = &sender {
                                let _ = sender.send(MultiAgentEvent::Runtime {
                                    id: event_id.clone(),
                                    event: Box::new(event),
                                });
                            }
                        })
                        .await;
                    if let Some(sender) = &sender {
                        let _ = sender.send(MultiAgentEvent::Finished { id: id.clone() });
                    }
                    AgentTaskResult { id, result }
                });
            }

            if let Some(result) = running.join_next().await {
                results.push(result.map_err(|error| AgentError::WorkerJoin(error.to_string()))?);
            }
        }
        results.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(results)
    }
}
