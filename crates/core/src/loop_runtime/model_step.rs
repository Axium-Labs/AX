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
        self.compress_if_needed(|event| (emit.lock().unwrap())(event))
            .await?;
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
        let response = self
            .request_with_retry(request, on_delta, on_thinking)
            .await?;
        drop(model_timer);
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
            && (self.stop_guard.is_some() || self.continuation.model_requests_continuation)
        {
            let marker = Message::system(if self.stop_guard.is_some() {
                "[ax-stop-guard-pending]"
            } else {
                "[ax-model-continuation]"
            });
            self.messages.push(marker.clone());
            self.raw_turn_messages.push(marker);
        }
        let mut assistant = Message::assistant(content.clone(), tool_calls.clone());
        assistant.provider_metadata = response.provider_metadata;
        assistant.usage = response.usage.map(|reported| {
            serde_json::json!({"provider": self.provider.name(), "model": self.provider.model_id(), "reported": reported})
        });
        self.messages.push(assistant.clone());
        self.raw_turn_messages.push(assistant);
        checkpoint(&self.raw_turn_messages)?;
        Ok(ModelStep {
            content,
            tool_calls,
            queue_active,
        })
    }
}
