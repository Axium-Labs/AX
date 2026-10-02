//! Per-turn goal lifecycle and durable queue checkpoints.
//!
//! Goal identity decides whether a turn starts, resumes or cancels work, and
//! every queue mutation is written through the caller's own checkpoint sink so
//! raw history stays authoritative.

use model::Message;

use super::state::AgentKernel;
use crate::{AgentError, AgentEvent, GoalTurn, QueueState, task_queue};

impl AgentKernel {
    pub(crate) fn begin_goal<H>(
        &mut self,
        input: &str,
        intent: GoalTurn,
        checkpoint: &mut H,
    ) -> Result<Option<String>, AgentError>
    where
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        if let Some(run) = &self.child_run {
            self.goal_id = Some(run.goal_id.clone());
            self.task_queue = None;
            return Ok(None);
        }
        let cancel = matches!(&intent, GoalTurn::Cancel { .. });
        match intent {
            GoalTurn::New | GoalTurn::Start { .. } => {
                let goal_id = if let GoalTurn::Start { goal_id } = intent {
                    goal_id
                } else {
                    task_queue::fresh_goal_id()
                };
                if goal_id.trim().is_empty()
                    || self
                        .task_queue
                        .as_ref()
                        .is_some_and(|q| q.goal_id == goal_id)
                {
                    return Err(AgentError::GoalMismatch("new goal ID must be nonempty and fresh; use explicit resume for the saved goal".into()));
                }
                let had_queue = self.task_queue.is_some();
                // A durable empty head prevents restoring an archived queue when
                // the replacement goal is a plain single task.
                let next = if had_queue {
                    let mut queue = task_queue::TaskQueue::new(input.into(), vec![]);
                    queue.goal_id.clone_from(&goal_id);
                    queue.parent_goal_id.clone_from(&self.parent_goal_id);
                    Some(queue)
                } else {
                    None
                };
                if let Some(previous) = &mut self.task_queue {
                    previous.stop(QueueState::Superseded, format!("superseded by {goal_id}"));
                    self.raw_turn_messages.push(Message::system(format!(
                        "{}{}",
                        task_queue::ARCHIVE_PREFIX,
                        serde_json::to_string(&previous).unwrap()
                    )));
                }
                self.goal_id = Some(goal_id);
                self.task_queue = next;
                self.set_context(task_queue::PROGRESS_PREFIX, None);
                self.messages
                    .retain(|m| !m.content.starts_with("[ax-recovery]"));
                // Supersede, archive and replacement head reach durable storage in
                // one checkpoint, so an interrupted turn cannot restore a
                // half-applied supersede. Raw history is still never truncated.
                if let Some(queue) = &self.task_queue {
                    self.raw_turn_messages.push(queue.snapshot());
                }
                checkpoint(&self.raw_turn_messages)?;
                Ok(None)
            }
            GoalTurn::Resume { goal_id } | GoalTurn::Cancel { goal_id } => {
                let queue = self
                    .task_queue
                    .as_mut()
                    .filter(|q| q.goal_id == goal_id)
                    .ok_or_else(|| {
                        AgentError::GoalMismatch(format!("no saved queue for goal {goal_id}"))
                    })?;
                if !matches!(
                    queue.state,
                    QueueState::Active | QueueState::Summarizing | QueueState::Suspended
                ) {
                    return Ok(Some(
                        queue
                            .final_response
                            .clone()
                            .or_else(|| queue.stop_reason.clone())
                            .unwrap_or_default(),
                    ));
                }
                if cancel {
                    queue.stop(QueueState::Cancelled, "Task queue canceled by user.".into());
                } else if queue.state == QueueState::Suspended {
                    queue.state = QueueState::Active;
                    queue.stop_reason = None;
                    queue.advance();
                }
                self.goal_id = Some(goal_id);
                self.checkpoint_queue(checkpoint)?;
                Ok(None)
            }
        }
    }

    pub(crate) fn finish_goal_response<F, H>(
        &mut self,
        content: String,
        emit: &std::sync::Mutex<F>,
        checkpoint: &mut H,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let queue = self.task_queue.as_mut().expect("terminal goal exists");
        if queue.final_response.is_some() {
            return Ok(content);
        }
        queue.final_response = Some(content.clone());
        queue.summarized = true;
        let message = Message::assistant(content.clone(), vec![]);
        self.messages.push(message.clone());
        self.raw_turn_messages.push(message);
        self.checkpoint_queue(checkpoint)?;
        (emit.lock().unwrap())(AgentEvent::ContentDelta {
            delta: content.clone(),
        });
        (emit.lock().unwrap())(AgentEvent::TurnFinished);
        Ok(content)
    }

    /// Durable queue checkpoint uses the caller's existing message persistence path.
    pub(crate) fn checkpoint_queue<H>(&mut self, checkpoint: &mut H) -> Result<(), AgentError>
    where
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        if let Some(queue) = &self.task_queue {
            self.raw_turn_messages.push(queue.snapshot());
            checkpoint(&self.raw_turn_messages)?;
        }
        Ok(())
    }
}
