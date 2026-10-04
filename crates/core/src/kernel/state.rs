//! The kernel's runtime state and the accessors that read or replace it.
//!
//! Fields are `pub(crate)` because the kernel is deliberately one object
//! shared by several cohesive modules — the loop, the child adapter, the
//! subagent adapter and the compression pipeline. External callers see only
//! the accessors below.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use model::{Message, ModelProvider};
use tool::ToolRegistry;

use crate::{
    ApprovalPolicy, ChildResult, ContextBudget, ContextPoolPolicy, ExecutionBudget, ExecutionState,
    QueueState, UserAnswer, UserQuestion,
    child::{ChildHost, ChildRun},
    subagent::{AgentTemplate, SubagentConfig, SubagentManager},
    task_queue,
    token::{estimate_tokens, estimate_tool_schema_tokens},
    user_input,
};

pub struct AgentKernel {
    pub(crate) coding_harness: bool,
    pub(crate) provider: Arc<dyn ModelProvider>,
    pub(crate) retry_policy: model::RetryPolicy,
    pub(crate) permission_profiles: Vec<tool::PermissionProfile>,
    pub(crate) child_models: HashMap<String, Arc<dyn ModelProvider>>,
    pub(crate) context_pool: ContextPoolPolicy,
    pub(crate) tools: ToolRegistry,
    pub(crate) approval: Arc<dyn ApprovalPolicy>,
    pub(crate) messages: Vec<Message>,
    pub(crate) budget: ExecutionBudget,
    pub(crate) raw_turn_messages: Vec<Message>,
    pub(crate) compression_dirty: bool,
    pub(crate) tool_concurrency: usize,
    pub(crate) result_reader: tool::ResultReader,
    pub(crate) task_queue: Option<task_queue::TaskQueue>,
    pub(crate) goal_id: Option<String>,
    pub(crate) parent_goal_id: Option<String>,
    pub(crate) child_host: Option<Arc<dyn ChildHost>>,
    pub(crate) child_run: Option<ChildRun>,
    pub(crate) child_budget: Option<ExecutionBudget>,
    pub(crate) child_concurrency: usize,
    /// Controller-side receipt index. Durable, never part of the request.
    pub(crate) child_results: std::collections::BTreeMap<String, ChildResult>,
    /// Set while a `request_user_input` question is unanswered.
    pub(crate) pending_question: Option<UserQuestion>,
    /// Whether the receipt index needs one durable write.
    pub(crate) receipts_dirty: bool,
    pub(crate) subagent_config: SubagentConfig,
    pub(crate) agent_templates: Vec<AgentTemplate>,
    pub(crate) subagent_manager: Option<Arc<SubagentManager>>,
    pub(crate) execution: Arc<std::sync::Mutex<ExecutionState>>,
    pub(crate) execution_root: Option<PathBuf>,
    /// Last time this kernel made observable progress. The turn timeout is an
    /// idle timeout: delegated children have their own budgets, so a long but
    /// productive batch must not be killed by a wall clock.
    pub(crate) progress_clock: Arc<std::sync::Mutex<std::time::Instant>>,
}

impl AgentKernel {
    /// Record observable progress: a model round, a tool round, a child
    /// dispatch or a child receipt.
    pub(crate) fn touch_progress(&self) {
        *self
            .progress_clock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = std::time::Instant::now();
    }

    #[must_use]
    pub fn execution_state(&self) -> ExecutionState {
        self.execution
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.get(name).is_some()
    }

    /// Names of the tools this kernel will actually send.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.names().into_iter().map(str::to_owned).collect()
    }

    /// Schema cost of exactly this kernel's registry.
    #[must_use]
    pub fn tool_schema_tokens(&self) -> usize {
        estimate_tool_schema_tokens(&self.tools)
    }

    /// Adds dynamically selected context, such as lazily loaded skill instructions.
    pub fn push_context(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Replace ephemeral retrieved context, rather than accumulating stale copies.
    pub fn set_context(&mut self, prefix: &str, message: Option<Message>) {
        self.messages
            .retain(|m| m.role != model::Role::System || !m.content.starts_with(prefix));
        if let Some(message) = message {
            self.messages.push(message);
        }
    }

    #[must_use]
    pub fn estimated_context_tokens(&self) -> usize {
        estimate_tokens(&self.messages)
    }

    #[must_use]
    pub fn context_budget(&self) -> ContextBudget {
        let mut budget = ContextBudget::new(
            self.provider.context_window(),
            self.provider.max_output_tokens(),
            estimate_tool_schema_tokens(&self.tools)
                + task_queue::schema_tokens()
                + crate::user_input::schema_tokens()
                + crate::child_result::schema_tokens(),
        );
        budget.pool = self.context_pool;
        budget
    }

    /// Context this kernel is about to add on top of the current estimate: the
    /// queue summary context is appended after the pressure check, and one
    /// bounded tool-result round may follow the request. The projection is the
    /// real pending addition, capped by the same policy value used elsewhere,
    /// rather than a private constant.
    #[must_use]
    pub fn pending_request_growth(&self, budget: ContextBudget) -> usize {
        let summary = self
            .task_queue
            .as_ref()
            .filter(|queue| queue.state == QueueState::Summarizing)
            .map_or(0, |queue| estimate_tokens(&[queue.summary_context()]));
        summary
            .min(budget.pool.tool_result_maximum)
            .saturating_add(budget.pool.next_request_reserve)
    }

    /// Raw messages produced by the last turn, independent of effective-context cleanup.
    pub fn take_turn_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.raw_turn_messages)
    }

    /// A snapshot is needed after compression so cleanup survives session resume.
    pub fn take_compression_dirty(&mut self) -> bool {
        std::mem::take(&mut self.compression_dirty)
    }

    #[must_use]
    pub fn goal_id(&self) -> Option<&str> {
        self.goal_id.as_deref()
    }

    #[must_use]
    pub fn parent_goal_id(&self) -> Option<&str> {
        self.parent_goal_id.as_deref()
    }

    #[must_use]
    pub fn task_queue_snapshot(&self) -> Option<Message> {
        self.task_queue
            .as_ref()
            .map(task_queue::TaskQueue::snapshot)
    }

    #[must_use]
    pub fn task_queue(&self) -> Option<&task_queue::TaskQueue> {
        self.task_queue.as_ref()
    }

    /// The question this run is parked on, when `request_user_input` suspended it.
    #[must_use]
    pub fn pending_question(&self) -> Option<&UserQuestion> {
        self.pending_question.as_ref()
    }

    /// Full child receipts produced during this goal, keyed by child id.
    #[must_use]
    pub fn child_results(&self) -> &std::collections::BTreeMap<String, ChildResult> {
        &self.child_results
    }

    #[must_use]
    pub fn child_result(&self, child_id: &str) -> Option<&ChildResult> {
        self.child_results.get(child_id).or_else(|| {
            self.child_results
                .values()
                .find(|receipt| receipt.task_id == child_id)
        })
    }

    /// Record a question raised by the model and persist its marker. Called
    /// before the run suspends, so the question survives a reconnect.
    pub(crate) fn ask_user(&mut self, question: UserQuestion) {
        self.pending_question = Some(question);
    }

    /// Write a structured answer back to the tool call that asked, then clear
    /// the pending question. The run resumes from that exact position: the
    /// answer becomes the tool result the model was waiting for.
    ///
    /// # Errors
    /// Returns a lifecycle error when no question is pending or the answer
    /// belongs to a different question.
    pub(crate) fn answer_user(
        &mut self,
        answer: &UserAnswer,
    ) -> Result<Message, crate::AgentError> {
        let question = self
            .pending_question
            .take()
            .ok_or_else(|| crate::AgentError::GoalMismatch("no pending user question".into()))?;
        if answer.question_id != question.id {
            self.pending_question = Some(question);
            return Err(crate::AgentError::GoalMismatch(format!(
                "answer targets `{}` but the pending question is `{}`",
                answer.question_id,
                self.pending_question.as_ref().unwrap().id
            )));
        }
        let payload = UserQuestion::answer_payload(&question, answer);
        let message = Message::tool(
            question.tool_call_id.clone(),
            serde_json::to_string(&payload).unwrap_or_default(),
        );
        self.messages.push(message.clone());
        self.raw_turn_messages.push(message.clone());
        Ok(message)
    }

    /// Durable question marker for the caller's checkpoint sink.
    pub(crate) fn question_marker(&self) -> Option<Message> {
        self.pending_question.as_ref().map(user_input::snapshot)
    }
}
