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
    ApprovalPolicy, ContextBudget, ContextPoolPolicy, ExecutionBudget, ExecutionState, QueueState,
    child::{ChildHost, ChildRun},
    subagent::{AgentTemplate, SubagentConfig, SubagentManager},
    task_queue,
    token::{estimate_tokens, estimate_tool_schema_tokens},
};

pub struct AgentKernel {
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
    pub(crate) subagent_config: SubagentConfig,
    pub(crate) agent_templates: Vec<AgentTemplate>,
    pub(crate) subagent_manager: Option<Arc<SubagentManager>>,
    pub(crate) execution: Arc<std::sync::Mutex<ExecutionState>>,
    pub(crate) execution_root: Option<PathBuf>,
}

impl AgentKernel {
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
            estimate_tool_schema_tokens(&self.tools) + task_queue::schema_tokens(),
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
}
