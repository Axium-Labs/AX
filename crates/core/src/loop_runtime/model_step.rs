//! One model step: prepare the request context, apply compression, call the
//! provider with retry and streaming, and record the assistant message.
//!
//! The step owns no loop control: it either returns the recorded response or a
//! structural error.

use model::{Message, ModelRequest, ToolCall, ToolSpec};

use crate::{
    AgentError, AgentEvent, AgentKernel, QueueState, context::request_context, execution,
    task_queue, token::estimate_tokens,
};

/// What one model step produced, in the form the following tool step needs.
pub(crate) struct ModelStep {
    /// Assistant text of the response.
    pub(crate) content: String,
    /// Tool calls the model requested; empty means the turn is finished.
    pub(crate) tool_calls: Vec<ToolCall>,
    /// Whether the task queue was active when the request was assembled.
    pub(crate) queue_active: bool,
}

impl AgentKernel {
    // Same exemption the inlined loop carried before it was extracted: one
    // model step is a single ordered transaction of context, retry and stream.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn model_step<F, H>(
        &mut self,
        emit: &std::sync::Mutex<F>,
        tool_specs: &[ToolSpec],
        steps_used: usize,
        checkpoint: &mut H,
    ) -> Result<ModelStep, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        // Tool results are re-projected for the request without changing what
        // the effective transcript keeps.
        let chars = self.context_budget().tool_result_chars();
        for message in &mut self.messages {
            if message.role == model::Role::Tool
                && let Ok(result) = serde_json::from_str::<tool::ToolResult>(&message.content)
            {
                message.content = result.model_view(chars);
            }
        }
        self.set_context(
            task_queue::PROGRESS_PREFIX,
            self.task_queue
                .as_ref()
                .filter(|q| q.active() && !q.tasks.is_empty())
                .map(task_queue::TaskQueue::progress),
        );
        let state = self.execution_state();
        let mut event_count = if state.progress.no_progress || steps_used == 1 {
            8
        } else {
            2
        };
        while event_count > 0
            && estimate_tokens(&[state.context(event_count)])
                > self.context_budget().session_summary_budget()
        {
            event_count -= 1;
        }
        self.set_context(execution::CONTEXT_PREFIX, Some(state.context(event_count)));
        if self.long_project_host.is_some() {
            self.checkpoint_goal_host().await?;
            self.project_context("state")?;
        }
        let compression = self
            .compress_if_needed(|event| (emit.lock().unwrap())(event))
            .await?;
        if compression.as_ref().is_some_and(|c| c.semantic_called)
            && let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut())
        {
            goal.usage.estimated = true;
            goal.usage.cost_estimated = true;
        }

        self.rotate_context(compression.as_ref(), checkpoint)?;
        (emit.lock().unwrap())(AgentEvent::ModelStarted {
            provider: self.provider.name().to_owned(),
            model: self.provider.model_id().to_owned(),
        });
        let queue_active = self
            .task_queue
            .as_ref()
            .is_some_and(|q| q.active() && !q.tasks.is_empty());
        let on_delta = |delta: String| {
            (emit.lock().unwrap())(AgentEvent::ContentDelta { delta });
        };
        let on_thinking = |delta: String| {
            (emit.lock().unwrap())(AgentEvent::ThinkingDelta { delta });
        };
        let model_timer = tool::telemetry::Timer::new("model.request");
        let mut request_history = self.messages.clone();
        if let Some(queue) = self
            .task_queue
            .as_ref()
            .filter(|q| matches!(q.state, QueueState::Summarizing | QueueState::Completed))
        {
            request_history.push(queue.summary_context());
            if let Some(receipts) = self.current_receipts_context() {
                request_history.push(receipts);
            }
        }
        let request_messages = request_context(&request_history, self.context_budget())?;
        let request = ModelRequest {
            messages: request_messages,
            tools: tool_specs.to_vec(),
        };
        let goal_enabled = self.goal().is_some();
        let input_estimate = if goal_enabled {
            estimate_tokens(&request.messages)
                + crate::token::estimate_text_tokens(
                    &serde_json::to_string(&request.tools).unwrap_or_default(),
                )
        } else {
            0
        };
        if goal_enabled {
            self.reserve_goal_checkpoint(input_estimate);
            self.checkpoint_queue(checkpoint)?;
            self.checkpoint_goal_host().await?;
        }
        let requests = std::sync::atomic::AtomicUsize::new(0);
        let response = if goal_enabled {
            self.request_with_retry_observed(request, on_delta, on_thinking, || {
                requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .await?
        } else {
            self.request_with_retry(request, on_delta, on_thinking)
                .await?
        };
        let attempts = requests.load(std::sync::atomic::Ordering::Relaxed);
        drop(model_timer);
        let output_estimate = if goal_enabled {
            crate::token::estimate_text_tokens(&response.content)
                + crate::token::estimate_text_tokens(
                    &serde_json::to_string(&response.tool_calls).unwrap_or_default(),
                )
        } else {
            0
        };
        if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
            goal.pending_usage = None;
            goal.record_usage(
                response.usage.as_ref(),
                u64::try_from(input_estimate).unwrap_or(u64::MAX),
                u64::try_from(output_estimate).unwrap_or(u64::MAX),
                attempts,
            );
        }
        self.sync_goal_usage();
        let content = response.content;
        let tool_calls = response.tool_calls;
        self.continuation.required_actions.clear();
        self.continuation.pending_tool_results = 0;
        self.continuation.unconsumed_child_results = 0;
        self.continuation.pending_tool_calls = tool_calls.len();
        self.continuation.model_requests_continuation = matches!(
            response.finish_reason.as_deref(),
            Some(
                "length"
                    | "max_tokens"
                    | "incomplete"
                    | "pause_turn"
                    | "tool_calls"
                    | "function_call"
            )
        );
        if tool_calls.is_empty()
            && (self.goal_evaluator.is_some() || self.continuation.model_requests_continuation)
        {
            let marker = Message::system(if self.goal_evaluator.is_some() {
                "[ax-goal-pending]"
            } else {
                "[ax-model-continuation]"
            });
            self.messages.push(marker.clone());
            self.raw_turn_messages.push(marker);
        }
        let mut assistant = Message::assistant(content.clone(), tool_calls.clone());
        assistant.provider_metadata = response.provider_metadata;
        assistant.usage = if goal_enabled {
            Some(
                serde_json::json!({"provider":self.provider.name(),"model":self.provider.model_id(),"reported":response.usage,"estimated_input_tokens":input_estimate,"estimated_output_tokens":output_estimate,"axGoalRequests":attempts}),
            )
        } else {
            response.usage.map(|reported| serde_json::json!({"provider":self.provider.name(),"model":self.provider.model_id(),"reported":reported}))
        };
        self.messages.push(assistant.clone());
        self.raw_turn_messages.push(assistant);
        checkpoint(&self.raw_turn_messages)?;
        if goal_enabled {
            self.checkpoint_queue(checkpoint)?;
            self.checkpoint_goal_host().await?;
        }
        if let Some(goal) = self.goal() {
            (emit.lock().unwrap())(AgentEvent::GoalUpdated {
                goal_id: self.goal_id.clone(),
                goal: Box::new(goal.clone()),
            });
        }
        Ok(ModelStep {
            content,
            tool_calls,
            queue_active,
        })
    }
}
