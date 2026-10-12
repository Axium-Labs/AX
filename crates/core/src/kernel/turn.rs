//! Per-turn goal lifecycle and durable queue checkpoints.
//!
//! Goal identity decides whether a turn starts, resumes or cancels work, and
//! every queue mutation is written through the caller's own checkpoint sink so
//! raw history stays authoritative.

use model::Message;

use super::state::AgentKernel;
use crate::{AgentError, AgentEvent, GoalTurn, QueueState, task_queue};

impl AgentKernel {
    // Goal admission is one atomic decision: intent, queue state and the
    // durable checkpoint must be written together, so the length is the point.
    #[allow(clippy::too_many_lines)]
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
        let pause = matches!(&intent, GoalTurn::Pause { .. });
        let config = match &intent {
            GoalTurn::Pursue { config, .. } => Some(config.clone()),
            GoalTurn::New | GoalTurn::Start { .. } => self.pending_goal.take(),
            _ => None,
        };
        match intent {
            GoalTurn::New | GoalTurn::Start { .. } | GoalTurn::Pursue { .. } => {
                let goal_id = match intent {
                    GoalTurn::Start { goal_id }
                    | GoalTurn::Pursue {
                        goal_id: Some(goal_id),
                        ..
                    } => goal_id,
                    _ => task_queue::fresh_goal_id(),
                };
                if goal_id.trim().is_empty()
                    || self
                        .task_queue
                        .as_ref()
                        .is_some_and(|q| q.goal_id == goal_id)
                {
                    return Err(AgentError::GoalMismatch("new goal ID must be nonempty and fresh; use explicit resume for the saved goal".into()));
                }
                // Every goal runs on an active queue: the harness drives the
                // queue to a terminal state, so a durable empty head prevents
                // restoring an archived queue when the replacement goal is a
                // plain single task.
                let mut queue = task_queue::TaskQueue::new(input.into(), vec![]);
                queue.goal = config
                    .filter(|c| c.mode != crate::GoalMode::Off)
                    .map(|c| crate::Goal::new(input.into(), c));
                queue.state = QueueState::Active;
                queue.goal_id.clone_from(&goal_id);
                queue.parent_goal_id.clone_from(&self.parent_goal_id);
                let next = Some(queue);
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
                let custom = self.pending_goal_evaluator.take();
                self.install_goal(self.goal().map(|g| g.config.clone()));
                if let Some(custom) = custom.filter(|_| self.goal().is_some()) {
                    self.task_queue
                        .as_mut()
                        .unwrap()
                        .goal
                        .as_mut()
                        .unwrap()
                        .evaluator_reference = Some(custom.policy_reference().into());
                    self.goal_evaluator = Some(custom);
                    self.wrap_goal_validators(self.goal().map(|g| g.config.clone()));
                }
                if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
                    goal.state = crate::GoalState::Running;
                    goal.elapsed_base_ms = goal.usage.elapsed_ms;
                    goal.active_since_ms = Some(crate::goal::now_ms());
                }
                self.set_context("[ax-goal-context]", None);
                self.set_context(task_queue::PROGRESS_PREFIX, None);
                self.messages.retain(|m| {
                    m.role != model::Role::System
                        || (!m.content.starts_with("[ax-recovery]")
                            && !m.content.starts_with("[ax-goal-")
                            && !m.content.starts_with("[ax-goal]"))
                });
                // Supersede, archive and replacement head reach durable storage in
                // one checkpoint, so an interrupted turn cannot restore a
                // half-applied supersede. Raw history is still never truncated.
                if let Some(queue) = &self.task_queue {
                    self.raw_turn_messages.push(queue.snapshot());
                }
                checkpoint(&self.raw_turn_messages)?;
                Ok(None)
            }
            GoalTurn::Resume { goal_id }
            | GoalTurn::Cancel { goal_id }
            | GoalTurn::Pause { goal_id } => {
                let verified_final = if !cancel
                    && !pause
                    && self
                        .goal()
                        .is_some_and(|g| g.last_decision == Some(crate::GoalDecision::Allow))
                {
                    crate::child::terminal_result(&self.messages).map(|r| r.summary)
                } else {
                    None
                };
                let queue = self
                    .task_queue
                    .as_mut()
                    .filter(|q| q.goal_id == goal_id)
                    .ok_or_else(|| {
                        AgentError::GoalMismatch(format!("no saved queue for goal {goal_id}"))
                    })?;
                let blocked_goal = queue
                    .goal
                    .as_ref()
                    .is_some_and(|g| g.state == crate::GoalState::Blocked);
                if !queue.resumable() && !blocked_goal {
                    return Ok(Some(
                        queue
                            .final_response
                            .clone()
                            .or_else(|| queue.stop_reason.clone())
                            .unwrap_or_default(),
                    ));
                }
                if let Some(content) = verified_final {
                    queue.state = QueueState::Completed;
                    queue.final_response = Some(content.clone());
                    queue.summarized = true;
                    if let Some(goal) = &mut queue.goal {
                        goal.state = crate::GoalState::Completed;
                        goal.stop_reason = None;
                    }
                    self.goal_id = Some(goal_id);
                    self.checkpoint_queue(checkpoint)?;
                    return Ok(Some(content));
                }
                if cancel {
                    if let Some(goal) = &mut queue.goal {
                        goal.state = crate::GoalState::Cancelled;
                        goal.stop_reason = Some("Cancelled by user".into());
                    }
                    if blocked_goal {
                        queue.state = QueueState::Active;
                    }
                    queue.stop(QueueState::Cancelled, "Task queue canceled by user.".into());
                } else if pause {
                    if let Some(goal) = &mut queue.goal {
                        goal.state = crate::GoalState::Paused;
                        goal.stop_reason = Some("Paused by user".into());
                    }
                    if blocked_goal {
                        queue.state = QueueState::Active;
                    }
                    queue.stop(QueueState::Suspended, "Goal paused by user.".into());
                } else if matches!(
                    queue.state,
                    QueueState::Suspended | QueueState::WaitingForUser | QueueState::Blocked
                ) {
                    // Resuming lifts the suspension; the queue itself is
                    // untouched, so no task state is lost.
                    queue.state = QueueState::Active;
                    queue.stop_reason = None;
                    queue.final_response = None;
                    if let Some(goal) = &mut queue.goal {
                        goal.state = crate::GoalState::Running;
                        goal.elapsed_base_ms = goal.usage.elapsed_ms;
                        goal.active_since_ms = Some(crate::goal::now_ms());
                        goal.stop_reason = None;
                    }
                    queue.advance();
                }
                self.goal_id = Some(goal_id);
                if !cancel
                    && !pause
                    && let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut())
                {
                    goal.state = crate::GoalState::Running;
                    goal.elapsed_base_ms = goal.usage.elapsed_ms;
                    goal.active_since_ms = Some(crate::goal::now_ms());
                }
                self.install_goal(self.goal().map(|g| g.config.clone()));
                self.checkpoint_queue(checkpoint)?;
                Ok(None)
            }
            GoalTurn::Answer { goal_id, .. } => {
                // A question can be asked with or without a task queue, so an
                // answer resumes the goal rather than requiring one.
                match self.task_queue.as_mut().filter(|q| q.goal_id == goal_id) {
                    Some(queue) => {
                        if !queue.resumable() {
                            return Ok(Some(
                                queue
                                    .final_response
                                    .clone()
                                    .or_else(|| queue.stop_reason.clone())
                                    .unwrap_or_default(),
                            ));
                        }
                        if matches!(
                            queue.state,
                            QueueState::Suspended | QueueState::WaitingForUser
                        ) {
                            queue.state = QueueState::Active;
                            queue.stop_reason = None;
                            queue.advance();
                        }
                    }
                    None if self.goal_id.as_deref() == Some(goal_id.as_str()) => {}
                    None => {
                        return Err(AgentError::GoalMismatch(format!(
                            "no saved goal {goal_id} to answer"
                        )));
                    }
                }
                self.goal_id = Some(goal_id);
                if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
                    goal.state = crate::GoalState::Running;
                    goal.elapsed_base_ms = goal.usage.elapsed_ms;
                    goal.active_since_ms = Some(crate::goal::now_ms());
                }
                self.install_goal(self.goal().map(|g| g.config.clone()));
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
        self.sync_goal_usage();
        if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
            for message in self
                .raw_turn_messages
                .iter()
                .filter(|m| m.role == model::Role::Tool)
            {
                if let Some(id) = &message.tool_call_id
                    && serde_json::from_str::<serde_json::Value>(&message.content)
                        .is_ok_and(|v| v["status"] == "success")
                {
                    goal.successful_calls.insert(id.clone());
                }
            }
        }
        if let Some(queue) = &self.task_queue {
            self.raw_turn_messages.push(queue.snapshot());
            checkpoint(&self.raw_turn_messages)?;
        }
        Ok(())
    }
}
