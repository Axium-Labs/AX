//! One tool round: decide what the assistant's tool calls mean and act on
//! them.
//!
//! Three outcomes exist, in the order the loop checks them: a final answer
//! with no tool calls, the `task_queue` control tool, or an ordinary tool
//! round scheduled through the DAG with bounded concurrency.

use std::sync::Arc;

use model::{Message, ToolCall};
use serde_json::Value;
use tokio::sync::mpsc;
use tool::ToolError;

use crate::{
    AgentError, AgentEvent, AgentKernel, QueueState, UserQuestion, child, child_result, scheduler,
    subagent, task_queue, user_input,
};

/// Whether the turn ended or needs another model step.
pub(crate) enum ToolStep {
    /// The turn produced its final answer.
    Final(String),
    /// The loop must run another model step.
    Continue,
    /// The run suspended on a user question and must be resumed with an answer.
    Waiting(Box<UserQuestion>),
}

impl AgentKernel {
    // Same exemption the inlined loop carried before it was extracted: the
    // three tool outcomes are guarded at one mutation boundary on purpose.
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn tool_step<F, H>(
        &mut self,
        emit: &std::sync::Mutex<F>,
        checkpoint: &mut H,
        mut content: String,
        mut tool_calls: Vec<ToolCall>,
        queue_active: bool,
        calls_used: &mut usize,
        steps_used: &mut usize,
        subagent_events: &mut Option<mpsc::UnboundedReceiver<AgentEvent>>,
    ) -> Result<ToolStep, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        if tool_calls.is_empty()
            && self.child_run.is_none()
            && self
                .task_queue
                .as_ref()
                .is_some_and(|q| q.state == QueueState::Active && !q.tasks.is_empty())
        {
            self.set_context("[ax-completion-guard]\n", Some(Message::system("[ax-completion-guard]\nKnown work is unresolved. Continue execution, dispatch ready children, or explicitly finish individual tasks with evidence. Use request_user_input for a user-exclusive blocker; use task_queue block only for an evidenced global blocker.")));
            self.checkpoint_queue(checkpoint)?;
            return Ok(ToolStep::Continue);
        }
        if tool_calls.is_empty() && self.coding_harness {
            if self.budget.max_steps != 0 && *steps_used >= self.budget.max_steps {
                return Err(AgentError::StepLimit(self.budget.max_steps));
            }
            *steps_used = steps_used.saturating_add(1);
            (emit.lock().unwrap())(AgentEvent::ModelStarted {
                provider: self.provider.name().into(),
                model: self.provider.model_id().into(),
            });
            let review = self.review_completion().await?;
            let mut message = Message::assistant(review.content.clone(), review.tool_calls.clone());
            message.usage = review.usage.map(|usage| serde_json::json!({"provider":self.provider.name(),"model":self.provider.model_id(),"reported":usage}));
            self.messages.push(message.clone());
            self.raw_turn_messages.push(message);
            let complete = review.tool_calls.len() == 1
                && review.tool_calls[0].function.name == crate::harness::CHECK
                && serde_json::from_str::<Value>(&review.tool_calls[0].function.arguments)
                    .is_ok_and(|v| v["state"] == "complete");
            if review
                .tool_calls
                .iter()
                .all(|call| call.function.name == crate::harness::CHECK)
            {
                for call in &review.tool_calls {
                    let result = Message::tool(&call.id, "Completion review recorded.");
                    self.messages.push(result.clone());
                    self.raw_turn_messages.push(result);
                }
                checkpoint(&self.raw_turn_messages)?;
                if !complete {
                    self.set_context(
                        "[ax-completion-guard]\n",
                        Some(Message::system(format!(
                            "[ax-completion-guard]\nContinue requested work. Review: {}",
                            serde_json::to_string(&review.tool_calls).unwrap()
                        ))),
                    );
                    return Ok(ToolStep::Continue);
                }
            } else {
                content = review.content;
                tool_calls = review.tool_calls;
            }
            checkpoint(&self.raw_turn_messages)?;
        }
        if tool_calls.is_empty() {
            if self.child_run.is_some()
                && let Some(outcome) = child::terminal_result(&self.messages)
                && !outcome.status.success()
            {
                return Err(AgentError::Tool(ToolError::Execution(
                    outcome.failure_reason.unwrap_or(outcome.summary),
                )));
            }
            if let Some(queue) = self.task_queue.as_mut().filter(|q| q.active()) {
                // A final response is goal-scoped. Never interpret it as one
                // task's completion and ask again for every remaining item.
                if queue.state == QueueState::Active && !queue.tasks.is_empty() {
                    queue.stop(QueueState::Blocked, content.clone());
                } else {
                    queue.state = QueueState::Completed;
                }
                queue.final_response = Some(content.clone());
                queue.summarized = true;
                self.checkpoint_queue(checkpoint)?;
                if queue_active || self.coding_harness {
                    (emit.lock().unwrap())(AgentEvent::ContentDelta {
                        delta: content.clone(),
                    });
                }
            }
            if self.coding_harness && self.task_queue.is_none() {
                (emit.lock().unwrap())(AgentEvent::ContentDelta {
                    delta: content.clone(),
                });
            }
            self.set_context(task_queue::PROGRESS_PREFIX, None);
            (emit.lock().unwrap())(AgentEvent::TurnFinished);
            return Ok(ToolStep::Final(content));
        }

        if self.budget.max_tool_calls != 0
            && tool_calls.len() > self.budget.max_tool_calls.saturating_sub(*calls_used)
        {
            return Err(AgentError::Budget("tool call limit".into()));
        }
        *calls_used = calls_used.saturating_add(tool_calls.len());
        if tool_calls
            .iter()
            .any(|call| call.function.name == user_input::TOOL_NAME)
        {
            return self.user_input_step(emit, checkpoint, &tool_calls);
        }
        if tool_calls
            .iter()
            .any(|call| call.function.name == child_result::TOOL_NAME)
        {
            return self.child_result_step(checkpoint, &tool_calls);
        }
        if tool_calls
            .iter()
            .any(|call| call.function.name == task_queue::TOOL_NAME)
        {
            let previous_queue = self.task_queue.clone();
            let queue_before = serde_json::to_string(&self.task_queue).unwrap();
            let result = if tool_calls.len() == 1 {
                serde_json::from_str::<Value>(&tool_calls[0].function.arguments)
                    .map_err(|error| error.to_string())
                    .and_then(|input| {
                        if self.coding_harness && input["action"] == "block" && !self.global_stop_evidenced(&input) {
                            return Err("Global stop requires an actual runtime global blocker. Local tool/skill/setup failures or untested resource assumptions cannot end the goal. Attempt the requested resource directly, recover setup, or record affected task failures and continue. User-exclusive decisions require request_user_input.".into());
                        }
                        task_queue::apply(
                            &mut self.task_queue,
                            &input,
                            self.goal_id.as_deref().unwrap_or_default(),
                            self.parent_goal_id.as_deref(),
                        )
                    })
            } else {
                Err("task_queue must be called alone; no tools in this round were executed".into())
            };
            if result.is_ok()
                && serde_json::from_str::<Value>(&tool_calls[0].function.arguments)
                    .is_ok_and(|input| input["action"] == "start")
                && let Some(mut previous) = previous_queue.filter(|q| !q.tasks.is_empty())
            {
                previous.stop(
                    QueueState::Superseded,
                    "replaced before task execution".into(),
                );
                self.raw_turn_messages.push(Message::system(format!(
                    "{}{}",
                    task_queue::ARCHIVE_PREFIX,
                    serde_json::to_string(&previous).unwrap()
                )));
            }
            for call in &tool_calls {
                let message = Message::tool(&call.id, match &result {
                    Ok(()) => "Queue updated. Continue current_task; summarize only after all tasks are terminal.".into(),
                    Err(reason) => format!("Queue update rejected: {reason}"),
                });
                let input = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
                let envelope = tool::ToolResult::new(result.is_ok(), message.content.clone());
                let advanced = result.is_ok()
                    && queue_before != serde_json::to_string(&self.task_queue).unwrap();
                self.execution.lock().unwrap().record_control(
                    &call.id,
                    &call.function.name,
                    &input,
                    &envelope,
                    advanced,
                );
                self.messages.push(message.clone());
                self.raw_turn_messages.push(message);
                self.raw_turn_messages
                    .push(self.execution_state().snapshot());
            }
            self.checkpoint_queue(checkpoint)?;
            checkpoint(&self.raw_turn_messages)?;
            return Ok(ToolStep::Continue);
        }
        let jobs = match scheduler::prepare(&tool_calls, &self.tools) {
            Ok(jobs) => jobs,
            Err(error) if queue_active => {
                for call in &tool_calls {
                    let message = Message::tool(
                        &call.id,
                        serde_json::to_string(&tool::ToolResult::new(false, error.to_string()))
                            .unwrap(),
                    );
                    self.messages.push(message.clone());
                    self.raw_turn_messages.push(message);
                }
                if let Some(task) = self
                    .task_queue
                    .as_mut()
                    .and_then(task_queue::TaskQueue::current_mut)
                {
                    task.failure_reason = Some(error.to_string());
                }
                self.checkpoint_queue(checkpoint)?;
                return Ok(ToolStep::Continue);
            }
            Err(error) => return Err(error),
        };
        if let Some(task) = self
            .task_queue
            .as_mut()
            .and_then(task_queue::TaskQueue::current_mut)
            && task.failure_reason.is_some()
        {
            task.recovery_attempts += 1;
        }
        if let Some(task) = self
            .task_queue
            .as_mut()
            .and_then(task_queue::TaskQueue::current_mut)
        {
            task.execution_started = true;
            self.checkpoint_queue(checkpoint)?;
        }
        let round_start = self.messages.len();
        let results = subagent::forward_events(
            scheduler::run(
                jobs,
                Arc::clone(&self.approval),
                self.permission_profiles.clone(),
                self.tool_concurrency,
                self.budget.tool_timeout_secs,
                emit,
                Some(Arc::clone(&self.execution)),
                |message, name, input| {
                    if let Some(tool) = self.tools.get(name) {
                        let result = serde_json::from_str::<tool::ToolResult>(&message.content)
                            .unwrap_or_else(|_| {
                                tool::ToolResult::new(true, message.content.clone())
                            });
                        self.execution.lock().unwrap().record(
                            message.tool_call_id.as_deref().unwrap_or_default(),
                            tool.as_ref(),
                            input,
                            &result,
                        );
                    } else {
                        let result = serde_json::from_str::<tool::ToolResult>(&message.content)
                            .unwrap_or_else(|_| {
                                tool::ToolResult::new(false, message.content.clone())
                            });
                        self.execution.lock().unwrap().record_control(
                            message.tool_call_id.as_deref().unwrap_or_default(),
                            name,
                            input,
                            &result,
                            false,
                        );
                    }
                    if let Some(id) = &message.tool_call_id
                        && let Ok(result) =
                            serde_json::from_str::<tool::ToolResult>(&message.content)
                    {
                        self.result_reader
                            .0
                            .write()
                            .unwrap()
                            .insert(id.clone(), result.raw_output);
                    }
                    let blocker = serde_json::from_str::<tool::ToolResult>(&message.content)
                        .ok()
                        .and_then(|result| result.global_blocker);
                    self.messages.push(message.clone());
                    self.raw_turn_messages.push(message);
                    self.raw_turn_messages
                        .push(self.execution_state().snapshot());
                    checkpoint(&self.raw_turn_messages)?;
                    if let Some(reason) = blocker {
                        return Err(AgentError::GlobalBlocked(reason));
                    }
                    Ok(())
                },
            ),
            subagent_events,
            emit,
        )
        .await?;
        // Raw checkpoints follow completion order; the next model request
        // receives one result per original call, in the original call order.
        self.messages.truncate(round_start);
        if let Some(task) = self
            .task_queue
            .as_mut()
            .and_then(task_queue::TaskQueue::current_mut)
        {
            let failures = results
                .iter()
                .filter_map(|m| serde_json::from_str::<tool::ToolResult>(&m.content).ok())
                .filter(|r| r.status != "success")
                .map(|r| r.raw_output)
                .collect::<Vec<_>>();
            task.failure_reason = if failures.is_empty() {
                None
            } else {
                Some(failures.join("\n"))
            };
        }
        for result in &results {
            if let Ok(envelope) = serde_json::from_str::<tool::ToolResult>(&result.content)
                && envelope.status == "success"
                && let Err(reason) = self.admit_work_items(&envelope.raw_output)
            {
                self.set_context("[ax-work-admission]\n", Some(Message::system(format!("[ax-work-admission]\nInvalid work inventory: {reason}. Correct complete task definitions."))));
            }
        }
        self.messages.extend(results);
        self.checkpoint_queue(checkpoint)?;
        Ok(ToolStep::Continue)
    }

    /// `request_user_input`: park the run on the question instead of ending it.
    ///
    /// The pending tool call is deliberately left unanswered so the answer can
    /// be written back to the same call id and execution resumes exactly here.
    fn user_input_step<F, H>(
        &mut self,
        emit: &std::sync::Mutex<F>,
        checkpoint: &mut H,
        tool_calls: &[ToolCall],
    ) -> Result<ToolStep, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let call = &tool_calls[0];
        let input: Value = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
        let parsed = (|| -> Result<UserQuestion, String> {
            if tool_calls.len() != 1 {
                return Err(
                    "request_user_input must be called alone; no tools in this round were executed"
                        .into(),
                );
            }
            user_input::parse_question(&input, &call.id)
        })();
        match parsed {
            Ok(question) => {
                let envelope = tool::ToolResult::new(
                    true,
                    "Suspended: the run resumes when the user answers.".to_owned(),
                );
                self.execution.lock().unwrap().record_control(
                    &call.id,
                    &call.function.name,
                    &input,
                    &envelope,
                    false,
                );
                self.ask_user(question.clone());
                (emit.lock().unwrap())(AgentEvent::UserQuestion {
                    question: Box::new(question.clone()),
                });
                Ok(ToolStep::Waiting(Box::new(question)))
            }
            Err(reason) => {
                let message = Message::tool(
                    &call.id,
                    serde_json::to_string(&tool::ToolResult::new(
                        false,
                        format!("request_user_input rejected: {reason}"),
                    ))
                    .unwrap_or_default(),
                );
                self.execution.lock().unwrap().record_control(
                    &call.id,
                    &call.function.name,
                    &input,
                    &tool::ToolResult::new(false, reason),
                    false,
                );
                self.messages.push(message.clone());
                self.raw_turn_messages.push(message);
                self.checkpoint_queue(checkpoint)?;
                checkpoint(&self.raw_turn_messages)?;
                Ok(ToolStep::Continue)
            }
        }
    }

    /// `child_result`: read a full child receipt without re-running the child.
    fn child_result_step<H>(
        &mut self,
        checkpoint: &mut H,
        tool_calls: &[ToolCall],
    ) -> Result<ToolStep, AgentError>
    where
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let alone = tool_calls.len() == 1;
        for call in tool_calls {
            let input: Value =
                serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
            let outcome = if alone {
                child_result::apply_read(&self.child_results, &input)
            } else {
                Err(
                    "child_result must be called alone; no tools in this round were executed"
                        .into(),
                )
            };
            let envelope = match &outcome {
                Ok(text) => tool::ToolResult::new(true, text.clone()),
                Err(reason) => tool::ToolResult::new(false, reason.clone()),
            };
            self.execution.lock().unwrap().record_control(
                &call.id,
                &call.function.name,
                &input,
                &envelope,
                false,
            );
            let message = Message::tool(
                &call.id,
                serde_json::to_string(&envelope).unwrap_or_default(),
            );
            self.messages.push(message.clone());
            self.raw_turn_messages.push(message);
        }
        self.checkpoint_queue(checkpoint)?;
        checkpoint(&self.raw_turn_messages)?;
        Ok(ToolStep::Continue)
    }
}
