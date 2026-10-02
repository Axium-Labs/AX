//! One user turn: goal admission, the model → tool → model loop, and the
//! durable checkpoints between them.
//!
//! The loop skeleton lives here on purpose. Its ordering — budget checks,
//! child delegation, one model step, one tool round — is the runtime's
//! contract, so it stays readable in a single place while the two steps
//! themselves live in [`model_step`] and [`tool_step`].

mod model_step;
mod tool_step;

#[cfg(test)]
mod retry_integration_tests;

pub(crate) use tool_step::ToolStep;

use model::{FunctionSpec, Message, ToolSpec};

use crate::{AgentError, AgentEvent, AgentKernel, GoalTurn, QueueState, scheduler, task_queue};

impl AgentKernel {
    /// Runs one user turn until the model returns a final response.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider or a tool fails structurally, tool
    /// arguments are invalid JSON, or a configured step limit is reached.
    ///
    /// # Panics
    ///
    /// Panics only if the event emitter's mutex is poisoned by a panic while
    /// a guard is held (unreachable in normal operation).
    pub async fn run_turn<F>(
        &mut self,
        input: impl Into<String>,
        emit: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.run_turn_checkpointed(input, emit, |_| Ok(())).await
    }

    /// Run with explicit goal identity, without requiring a new session for workers.
    ///
    /// # Errors
    /// Returns lifecycle, execution or persistence failures.
    pub async fn run_goal_turn<F>(
        &mut self,
        input: impl Into<String>,
        intent: GoalTurn,
        emit: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.run_goal_turn_checkpointed(input, intent, emit, |_| Ok(()))
            .await
    }

    /// Persist each complete message before advancing model or tool execution.
    ///
    /// # Errors
    /// Returns runtime or checkpoint failures. Already saved messages are not replayed.
    pub async fn run_turn_checkpointed<F, H>(
        &mut self,
        input: impl Into<String>,
        emit: F,
        checkpoint: H,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        self.run_goal_turn_checkpointed(input, GoalTurn::New, emit, checkpoint)
            .await
    }

    /// Resume/cancel requires the exact saved goal ID. Ordinary calls start new goals.
    ///
    /// # Errors
    /// Returns lifecycle, runtime or checkpoint failures.
    pub async fn run_goal_turn_checkpointed<F, H>(
        &mut self,
        input: impl Into<String>,
        intent: GoalTurn,
        mut emit: F,
        mut checkpoint: H,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        self.raw_turn_messages.clear();
        let result = if self.budget.turn_timeout_secs == 0 {
            self.run_turn_inner(input, intent, &mut emit, &mut checkpoint)
                .await
        } else {
            tokio::time::timeout(
                std::time::Duration::from_secs(self.budget.turn_timeout_secs),
                self.run_turn_inner(input, intent, &mut emit, &mut checkpoint),
            )
            .await
            .unwrap_or_else(|_| Err(AgentError::Budget("turn timeout".into())))
        };
        if result
            .as_ref()
            .is_err_and(|error| !matches!(error, AgentError::GoalMismatch(_)))
        {
            if let Some(queue) = &mut self.task_queue {
                if matches!(
                    &result,
                    Err(AgentError::Budget(_) | AgentError::StepLimit(_))
                ) {
                    queue.stop(
                        QueueState::Suspended,
                        result.as_ref().unwrap_err().to_string(),
                    );
                } else {
                    let reason = result.as_ref().unwrap_err().to_string();
                    queue.stop(QueueState::Blocked, reason.clone());
                    queue.final_response.get_or_insert(reason);
                }
            }
            // Persist a valid tool-call transcript even if a deadline interrupts execution.
            let answered = self
                .messages
                .iter()
                .filter_map(|m| m.tool_call_id.clone())
                .collect::<std::collections::HashSet<_>>();
            let pending = self
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .filter(|call| !answered.contains(&call.id))
                .map(|call| call.id.clone())
                .collect::<Vec<_>>();
            for id in pending {
                let interrupted = Message::tool(
                    id,
                    "Execution interrupted before a tool result was available.",
                );
                self.messages.push(interrupted.clone());
                self.raw_turn_messages.push(interrupted);
            }
        }
        if result
            .as_ref()
            .is_err_and(|error| !matches!(error, AgentError::GoalMismatch(_)))
        {
            self.checkpoint_queue(&mut checkpoint)?;
        }
        checkpoint(&self.raw_turn_messages)?;
        result
    }

    // The loop skeleton stays in one place: its ordering is the runtime's
    // contract, so it is exempted from the line budget the way it was before
    // the model and tool steps were extracted.
    #[allow(clippy::too_many_lines)]
    async fn run_turn_inner<F, H>(
        &mut self,
        input: impl Into<String>,
        intent: GoalTurn,
        emit: F,
        checkpoint: &mut H,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let emit = std::sync::Mutex::new(emit);
        let input = input.into();
        let new_goal = matches!(&intent, GoalTurn::New | GoalTurn::Start { .. });
        let replay = self.begin_goal(&input, intent, checkpoint)?;
        if self.execution.lock().unwrap().overall_goal.is_empty()
            || (new_goal && self.child_run.is_none())
        {
            let cwd = self.child_run.as_ref().map_or_else(
                || {
                    self.execution_root
                        .clone()
                        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
                },
                |run| run.cwd.clone(),
            );
            self.execution.lock().unwrap().begin(
                &input,
                self.goal_id.as_deref().unwrap_or_default(),
                &cwd,
            );
        }
        if let Some(cached) = replay {
            return Ok(cached);
        }
        (emit.lock().unwrap())(AgentEvent::TurnStarted);
        if self.child_run.is_none() || !self.messages.iter().any(|m| m.role == model::Role::User) {
            let user = Message::user(input);
            self.messages.push(user.clone());
            self.raw_turn_messages.push(user);
        }
        if self.child_run.is_some() {
            // Never replay an interrupted call whose side effects are unknown.
            let answered = self
                .messages
                .iter()
                .filter_map(|m| m.tool_call_id.clone())
                .collect::<std::collections::HashSet<_>>();
            let pending = self
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .filter(|call| !answered.contains(&call.id))
                .map(|call| call.id.clone())
                .collect::<Vec<_>>();
            for id in pending {
                let message = Message::tool(
                    id,
                    "Interrupted child call; result and side effects are unknown. Inspect workspace state before retrying.",
                );
                self.messages.push(message.clone());
                self.raw_turn_messages.push(message);
            }
        }
        self.raw_turn_messages
            .push(self.execution_state().snapshot());
        checkpoint(&self.raw_turn_messages)?;
        self.checkpoint_queue(checkpoint)?;
        let mut subagent_events = self.prepare_subagents();
        let mut tool_specs = self
            .tools
            .iter()
            .map(|tool| ToolSpec {
                kind: "function",
                function: FunctionSpec {
                    name: tool.name().to_owned(),
                    description: tool.description().to_owned(),
                    parameters: scheduler::input_schema(tool.input_schema()),
                },
            })
            .collect::<Vec<_>>();

        if self.child_run.is_none() {
            tool_specs.push(task_queue::spec());
        }

        let mut calls_used = 0;
        let mut steps_used = 0usize;
        loop {
            if let Some(queue) = self.task_queue.as_ref().filter(|q| !q.active()) {
                let content = queue
                    .final_response
                    .clone()
                    .or_else(|| queue.stop_reason.clone())
                    .unwrap_or_default();
                return self.finish_goal_response(content, &emit, checkpoint);
            }
            if self.budget.max_steps != 0 && steps_used >= self.budget.max_steps {
                return Err(AgentError::StepLimit(self.budget.max_steps));
            }
            steps_used = steps_used.saturating_add(1);
            if self.child_host.is_some()
                && self.task_queue.as_ref().is_some_and(|q| {
                    q.delegate
                        && q.state == QueueState::Active
                        && q.tasks
                            .iter()
                            .any(|t| t.status == task_queue::TaskStatus::Running)
                })
            {
                self.execute_next_child(&emit, checkpoint).await?;
                continue;
            }
            let step = self
                .model_step(&emit, &tool_specs, steps_used, checkpoint)
                .await?;
            let queue_active = step.queue_active;
            match self
                .tool_step(
                    &emit,
                    checkpoint,
                    step.content,
                    step.tool_calls,
                    queue_active,
                    &mut calls_used,
                    &mut subagent_events,
                )
                .await?
            {
                ToolStep::Final(content) => return Ok(content),
                ToolStep::Continue => {}
            }
        }
    }
}
