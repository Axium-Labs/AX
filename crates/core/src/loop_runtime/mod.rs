//! One user turn: goal admission, the model → tool → model loop, and the
//! durable checkpoints between them.
//!
//! The loop skeleton lives here on purpose. Its ordering — budget checks,
//! child delegation, one model step, one tool round — is the runtime's
//! contract, so it stays readable in a single place while the two steps
//! themselves live in [`model_step`] and [`tool_step`].

mod model_step;
mod provider_step;
mod tool_step;

#[cfg(test)]
mod retry_integration_tests;

pub(crate) use tool_step::ToolStep;

use model::{FunctionSpec, Message, ToolSpec};

use crate::{
    AgentError, AgentEvent, AgentKernel, GoalTurn, QueueState, child_dispatch, child_result,
    scheduler, task_queue, user_input,
};

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
    // Keep the persistence, timeout and extension completion boundaries together.
    #[allow(clippy::too_many_lines)]
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
        if self.turn_input.closed() {
            self.turn_input = crate::TurnInput::default();
        }
        let _input_lease = crate::continuation::TurnInputLease(self.turn_input.clone());
        self.raw_turn_messages.clear();
        self.continuation = crate::TurnState::default();
        self.goal_model_requests = 0;
        self.repeat_calls.reset();
        self.prepare_environment().await?;
        self.touch_progress();
        // The controller turn timeout is an *idle* timeout. A turn that delegates work
        // legitimately spans the whole child batch, and each child already has
        // its own budget, so the controller is only cancelled when nothing has
        // progressed for the whole window.
        let enforce_goal_deadline =
            !matches!(&intent, GoalTurn::Pause { .. } | GoalTurn::Cancel { .. })
                && (matches!(
                    &intent,
                    GoalTurn::New | GoalTurn::Start { .. } | GoalTurn::Pursue { .. }
                ) || self.goal().is_some_and(|g| {
                    !matches!(
                        g.state,
                        crate::GoalState::Completed
                            | crate::GoalState::Failed
                            | crate::GoalState::Cancelled
                    )
                }));
        let goal_config = match &intent {
            GoalTurn::Pursue { config, .. } => Some(config.clone()),
            GoalTurn::New | GoalTurn::Start { .. } => self.pending_goal.clone(),
            _ => self.goal().map(|g| g.config.clone()),
        }
        .filter(|config| config.mode != crate::GoalMode::Off);
        if goal_config.is_some() {
            self.turn_input.enable_goal_control();
        }
        let saved_ms = if matches!(&intent, GoalTurn::Resume { .. } | GoalTurn::Answer { .. }) {
            self.goal().map_or(0, |g| g.usage.elapsed_ms)
        } else {
            0
        };
        let original_budget = self.budget;
        if enforce_goal_deadline
            && let Some(config) = &goal_config
            && config.max_rounds > 0
        {
            let consumed = if matches!(&intent, GoalTurn::Resume { .. } | GoalTurn::Answer { .. }) {
                self.goal().map_or(0, |g| g.usage.rounds)
            } else {
                0
            };
            let remaining = config.max_rounds.saturating_sub(consumed).max(1);
            self.budget.max_steps = if self.budget.max_steps == 0 {
                remaining
            } else {
                self.budget.max_steps.min(remaining)
            };
        }
        let goal_window = goal_config
            .as_ref()
            .filter(|c| enforce_goal_deadline && c.max_seconds > 0)
            .map(|c| {
                std::time::Duration::from_millis(
                    c.max_seconds
                        .saturating_mul(1000)
                        .saturating_sub(saved_ms)
                        .max(1),
                )
            });
        let control = self.turn_input.clone();
        let execution = async {
            if self.budget.turn_timeout_secs == 0 && goal_window.is_none() {
                self.run_turn_inner(input, intent, &mut emit, &mut checkpoint)
                    .await
            } else if self.child_run.is_some() || goal_window.is_some() {
                // An explicitly budgeted isolated child must not extend its deadline
                // by repeatedly emitting model/tool activity. Its failure stays local.
                let window = goal_window.map_or_else(
                    || std::time::Duration::from_secs(self.budget.turn_timeout_secs),
                    |window| {
                        if self.budget.turn_timeout_secs == 0 {
                            window
                        } else {
                            window.min(std::time::Duration::from_secs(
                                self.budget.turn_timeout_secs,
                            ))
                        }
                    },
                );
                tokio::time::timeout(
                    window,
                    self.run_turn_inner(input, intent, &mut emit, &mut checkpoint),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(AgentError::Timeout(
                        if goal_window.is_some() {
                            "Goal execution timeout"
                        } else {
                            "child execution timeout"
                        }
                        .into(),
                    ))
                })
            } else {
                let window = std::time::Duration::from_secs(self.budget.turn_timeout_secs);
                let clock = std::sync::Arc::clone(&self.progress_clock);
                let mut inner =
                    std::pin::pin!(self.run_turn_inner(input, intent, &mut emit, &mut checkpoint));
                loop {
                    if let Ok(result) = tokio::time::timeout(window, inner.as_mut()).await {
                        break result;
                    }
                    let idle = clock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .elapsed();
                    if idle < window {
                        continue;
                    }
                    break Err(AgentError::Timeout("turn timeout".into()));
                }
            }
        };
        let result = if goal_config.is_some() {
            let interrupted = async {
                loop {
                    if let Some(state) = control.goal_control() {
                        break AgentError::GoalStopped {
                            state,
                            reason: "User requested Goal control".into(),
                        };
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            };
            tokio::select! {
                result = execution => result,
                error = interrupted => Err(error),
            }
        } else {
            execution.await
        };
        self.budget = original_budget;
        let mismatched = matches!(&result, Err(AgentError::GoalMismatch(_)));
        if !mismatched {
            self.sync_goal_usage();
            if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
                goal.active_since_ms = None;
                if result.is_err()
                    && let Some(mut usage) = goal.pending_usage.take()
                {
                    usage.elapsed_ms = goal.usage.elapsed_ms;
                    goal.usage = usage;
                }
            }
        }
        if result
            .as_ref()
            .is_err_and(|error| !matches!(error, AgentError::GoalMismatch(_)))
        {
            // Suspension is not failure: a budget, step limit, timeout or user
            // question leaves the goal resumable and never drops the queue.
            let waiting = matches!(&result, Err(AgentError::WaitingForUser(_)));
            if let Some(queue) = &mut self.task_queue {
                if let Some(goal) = &mut queue.goal {
                    goal.state = match result.as_ref().unwrap_err() {
                        AgentError::GoalStopped { state, .. } => *state,
                        AgentError::Model(error)
                            if error.error_class() == model::ErrorClass::Retryable =>
                        {
                            crate::GoalState::Paused
                        }
                        AgentError::GlobalBlocked(_)
                        | AgentError::Model(
                            model::ModelError::HttpStatus {
                                status: 401 | 403, ..
                            }
                            | model::ModelError::HttpResponse {
                                status: 401 | 403, ..
                            }
                            | model::ModelError::Configuration(_),
                        ) => crate::GoalState::Blocked,
                        error if error.resumable() => crate::GoalState::Paused,
                        _ => crate::GoalState::Failed,
                    };
                    goal.stop_reason = Some(result.as_ref().unwrap_err().to_string());
                }
                if waiting {
                    queue.await_user();
                } else if result.as_ref().is_err_and(AgentError::resumable) {
                    queue.stop(
                        QueueState::Suspended,
                        result.as_ref().unwrap_err().to_string(),
                    );
                } else {
                    let reason = result.as_ref().unwrap_err().to_string();
                    queue.stop(QueueState::Blocked, reason.clone());
                    queue.final_response.get_or_insert(reason);
                }
                if queue.goal.is_some()
                    && queue
                        .goal
                        .as_ref()
                        .is_some_and(|g| g.state == crate::GoalState::Paused)
                {
                    queue.state = QueueState::Suspended;
                }
                if queue
                    .goal
                    .as_ref()
                    .is_some_and(|g| g.state == crate::GoalState::Cancelled)
                {
                    queue.state = QueueState::Cancelled;
                }
            }
            if waiting {
                // The pending call stays unanswered on purpose: the question
                // marker is what lets a reconnected session still answer it.
                if let Some(marker) = self.question_marker() {
                    self.raw_turn_messages.push(marker);
                }
            } else {
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
        }
        if result
            .as_ref()
            .is_err_and(|error| !matches!(error, AgentError::GoalMismatch(_)))
        {
            self.checkpoint_queue(&mut checkpoint)?;
        }
        self.turn_input.close();
        if goal_config.is_some() && !mismatched {
            self.checkpoint_queue(&mut checkpoint)?;
            self.checkpoint_goal_host().await?;
            if result.is_err()
                && let Some(goal) = self.goal()
            {
                emit(AgentEvent::GoalUpdated {
                    goal_id: self.goal_id.clone(),
                    goal: Box::new(goal.clone()),
                });
            }
        }
        // Preserve accepted guidance even when the provider fails before the next boundary.
        for message in self.turn_input.drain() {
            self.messages.push(message.clone());
            self.raw_turn_messages.push(message);
        }
        checkpoint(&self.raw_turn_messages)?;
        if let Some(extension) = &self.extension {
            let error = result.as_ref().err().map(ToString::to_string);
            let completion = async {
                extension
                    .sync_context(&self.messages, self.provider.context_window())
                    .await?;
                extension
                    .after_turn(result.as_ref().ok().map(String::as_str), error.as_deref())
                    .await
            }
            .await;
            if result.is_ok() {
                completion?;
            }
        }
        if self.long_project_host.is_some() && !mismatched {
            self.project_context("state")?;
        }
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
        let new_goal = matches!(
            &intent,
            GoalTurn::New | GoalTurn::Start { .. } | GoalTurn::Pursue { .. }
        );
        let answer = match &intent {
            GoalTurn::Answer { answer, .. } => Some(answer.clone()),
            _ => None,
        };
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
                |run| run.workspace_root().to_path_buf(),
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
        if self.goal().is_some() {
            self.prepare_goal_validators()?;
            self.checkpoint_queue(checkpoint)?;
            self.checkpoint_goal_host().await?;
        }
        (emit.lock().unwrap())(AgentEvent::TurnStarted);
        if let Some(goal) = self.goal() {
            (emit.lock().unwrap())(AgentEvent::GoalUpdated {
                goal_id: self.goal_id.clone(),
                goal: Box::new(goal.clone()),
            });
            let context = Message::system(format!(
                "[ax-goal-context]\n{}",
                serde_json::json!({"goal_id":self.goal_id,"target":goal.target,"acceptance":goal.config.acceptance,"deliverables":goal.config.deliverables,"required_successful_calls":goal.config.required_successful_calls,"recovery":"Inspect recorded results and external state before retrying interrupted operations. Never blindly replay side effects."})
            ));
            self.set_context("[ax-goal-context]", Some(context));
        }
        if let Some(answer) = &answer {
            // Resume from the exact position the question was asked: the answer
            // becomes the tool result the model was waiting for, and no new
            // user turn is opened.
            self.answer_user(answer)?;
            checkpoint(&self.raw_turn_messages)?;
        } else if self.child_run.is_none()
            || !self.messages.iter().any(|m| m.role == model::Role::User)
        {
            self.raw_turn_messages.push(Message::user(input.clone()));
            if let Some(extension) = &self.extension {
                extension
                    .sync_context(&self.messages, self.provider.context_window())
                    .await?;
                let adapted = extension.before_turn(&input).await?;
                self.messages.push(Message::user(adapted.text));
                for context in adapted.context {
                    let message = Message::system(format!("[ax-mod-context]\n{context}"));
                    self.messages.push(message.clone());
                    self.raw_turn_messages.push(message);
                }
                if let Some(response) = adapted.response {
                    if self.goal().is_some() {
                        self.messages.push(Message::system(format!("[ax-goal-mod-result]\n{response}\nThis result still requires Goal verification.")));
                    } else {
                        if let Some(queue) = &mut self.task_queue {
                            queue.stop(QueueState::Completed, "Mod command completed".into());
                        }
                        return self.finish_goal_response(response, &emit, checkpoint);
                    }
                }
            } else {
                self.messages.push(Message::user(input));
            }
        }
        if self.child_run.is_some() || self.goal().is_some() {
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
            // Controller-only control tools. Children share the registry but not
            // these: they own no queue and cannot park the controller's run.
            tool_specs.push(task_queue::spec());
            tool_specs.push(user_input::spec());
            if self.child_host.is_some() || !self.child_results.is_empty() {
                tool_specs.push(child_result::spec());
            }
        }

        let mut calls_used = 0;
        let mut steps_used = 0usize;
        loop {
            let guidance = self.turn_input.drain();
            if !guidance.is_empty()
                && let Some(queue) = self
                    .task_queue
                    .as_mut()
                    .filter(|q| q.state == QueueState::Completed)
            {
                queue.state = QueueState::Active;
                queue.final_response = None;
                queue.summarized = false;
            }
            for message in guidance {
                self.messages.push(message.clone());
                self.raw_turn_messages.push(message);
                checkpoint(&self.raw_turn_messages)?;
            }
            self.touch_progress();
            if let Some(queue) = self.task_queue.as_ref().filter(|q| !q.active()) {
                if self.goal().is_some_and(|g| {
                    matches!(
                        g.state,
                        crate::GoalState::Paused | crate::GoalState::Cancelled
                    )
                }) {
                    return Err(AgentError::GoalStopped {
                        state: self.goal().unwrap().state,
                        reason: queue.stop_reason.clone().unwrap_or_default(),
                    });
                }
                if self.goal().is_some() && queue.state == QueueState::Blocked {
                    return Err(AgentError::GlobalBlocked(
                        queue.stop_reason.clone().unwrap_or_default(),
                    ));
                }
                // A terminal control action closes input at the same admission boundary.
                if !self.turn_input.close_if_empty() {
                    continue;
                }
                let content = queue
                    .final_response
                    .clone()
                    .or_else(|| queue.stop_reason.clone())
                    .unwrap_or_default();
                return self.finish_goal_response(content, &emit, checkpoint);
            }
            self.goal_budget_check()?;
            if let Some(goal) = self.goal() {
                if goal.config.max_rounds > 0 && goal.usage.rounds >= goal.config.max_rounds {
                    return Err(AgentError::Budget("Goal model-round limit".into()));
                }
                if goal.config.max_seconds > 0
                    && goal.usage.elapsed_ms >= goal.config.max_seconds.saturating_mul(1000)
                {
                    return Err(AgentError::Budget("Goal time limit".into()));
                }
            }
            if self.budget.max_steps != 0
                && steps_used.saturating_add(self.goal_model_requests) >= self.budget.max_steps
            {
                return Err(AgentError::StepLimit(self.budget.max_steps));
            }
            if self.child_host.is_some()
                && self.task_queue.as_ref().is_some_and(|q| {
                    q.delegate && q.state == QueueState::Active && child_dispatch::has_open_work(q)
                })
            {
                // Dispatch the whole ready frontier at once; children that are
                // independent run together, dependents wait for their
                // predecessor, and one failure never stops the others.
                if self.execute_ready_children(&emit, checkpoint).await? > 0 {
                    if self.goal().is_some() {
                        self.checkpoint_queue(checkpoint)?;
                    }
                    self.observe_goal_progress()?;
                    continue;
                }
            }
            steps_used = steps_used.saturating_add(1);
            if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
                goal.usage.rounds = goal.usage.rounds.saturating_add(1);
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
                    &mut steps_used,
                    &mut subagent_events,
                )
                .await?
            {
                ToolStep::Final(content) => return Ok(content),
                ToolStep::Continue => {
                    self.observe_goal_progress()?;
                    if self.goal().is_some() {
                        self.checkpoint_queue(checkpoint)?;
                    }
                    self.continuation.pending_tool_calls = 0;
                    self.continuation.pending_tool_results = self
                        .messages
                        .iter()
                        .rev()
                        .take_while(|message| message.role != model::Role::Assistant)
                        .filter(|message| message.role == model::Role::Tool)
                        .count();
                }
                ToolStep::Waiting(question) => {
                    return Err(AgentError::WaitingForUser(question));
                }
            }
        }
    }
}
