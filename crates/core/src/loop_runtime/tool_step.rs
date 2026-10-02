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
    AgentError, AgentEvent, AgentKernel, QueueState, child, scheduler, subagent, task_queue,
};

/// Whether the turn ended or needs another model step.
pub(crate) enum ToolStep {
    /// The turn produced its final answer.
    Final(String),
    /// The loop must run another model step.
    Continue,
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
        content: String,
        tool_calls: Vec<ToolCall>,
        queue_active: bool,
        calls_used: &mut usize,
        subagent_events: &mut Option<mpsc::UnboundedReceiver<AgentEvent>>,
    ) -> Result<ToolStep, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        if tool_calls.is_empty() {
            if self.child_run.is_some()
                && let Some(outcome) = child::terminal_outcome(&self.messages)
                && !outcome.success
            {
                return Err(AgentError::Tool(ToolError::Execution(outcome.output)));
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
                if queue_active {
                    (emit.lock().unwrap())(AgentEvent::ContentDelta {
                        delta: content.clone(),
                    });
                }
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
            .any(|call| call.function.name == task_queue::TOOL_NAME)
        {
            let previous_queue = self.task_queue.clone();
            let queue_before = serde_json::to_string(&self.task_queue).unwrap();
            let result = if tool_calls.len() == 1 {
                serde_json::from_str::<Value>(&tool_calls[0].function.arguments)
                    .map_err(|error| error.to_string())
                    .and_then(|input| {
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
        self.messages.extend(results);
        self.checkpoint_queue(checkpoint)?;
        Ok(ToolStep::Continue)
    }
}
