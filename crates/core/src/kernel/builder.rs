//! How an [`AgentKernel`] is constructed and configured before a turn.
//!
//! Every builder method is lazy: it only records configuration, so the
//! composition root can wire a kernel without triggering provider, skill, MCP
//! or memory work.

use std::sync::Arc;

use model::{Message, ModelProvider};
use tool::ToolRegistry;

use super::state::AgentKernel;
use crate::{
    ApprovalPolicy, ContextPoolPolicy, ExecutionBudget, ExecutionState, SubagentConfig,
    child_result, task_queue, user_input,
};

impl AgentKernel {
    #[must_use]
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        mut tools: ToolRegistry,
        approval: Arc<dyn ApprovalPolicy>,
    ) -> Self {
        let result_reader = tool::ResultReader::default();
        tool::kernel_baseline(&mut tools, provider.capabilities().vision);
        // The kernel binds its own reader so children can rebind it.
        tools.register(result_reader.clone());
        Self {
            extension: None,
            guard_model_requests: 0,
            continuation: crate::TurnState::default(),
            activity: Arc::default(),
            turn_input: crate::continuation::TurnInput::default(),
            stop_guard: None,
            provider,
            retry_policy: model::RetryPolicy::default(),
            permission_profiles: vec![],
            child_models: std::collections::HashMap::new(),
            context_pool: ContextPoolPolicy::default(),
            tools,
            approval,
            messages: Vec::new(),
            budget: ExecutionBudget::default(),
            raw_turn_messages: Vec::new(),
            compression_dirty: false,
            tool_concurrency: 4,
            result_reader,
            task_queue: None,
            goal_id: None,
            parent_goal_id: None,
            child_host: None,
            child_run: None,
            child_budget: None,
            child_concurrency: 4,
            child_results: std::collections::BTreeMap::new(),
            pending_question: None,
            receipts_dirty: false,
            subagent_config: SubagentConfig::default(),
            subagent_depth: 0,
            subagent_pool: None,
            subagent_tools: vec!["subagent".into(), "subagent_fork".into()],
            agent_templates: Vec::new(),
            subagent_manager: None,
            repeat_calls: crate::loop_hygiene::RepeatCallChain::default(),
            execution: Arc::new(std::sync::Mutex::new(ExecutionState::fresh())),
            execution_root: None,
            progress_clock: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        }
    }

    #[must_use]
    pub fn with_execution_scope(mut self, root: std::path::PathBuf) -> Self {
        self.execution_root = Some(root);
        self
    }

    #[must_use]
    pub fn with_execution_budget(mut self, budget: ExecutionBudget) -> Self {
        self.budget = budget;
        self
    }

    pub fn configure_context_pool(&mut self, policy: ContextPoolPolicy) {
        self.context_pool = policy;
    }

    pub fn register_child_model(&mut self, name: String, provider: Arc<dyn ModelProvider>) {
        self.child_models.insert(name, provider);
    }

    pub fn constrain_permissions(&mut self, profile: tool::PermissionProfile) {
        self.permission_profiles.push(profile);
    }

    pub fn configure_retry(&mut self, policy: model::RetryPolicy) {
        self.retry_policy = policy;
    }

    #[must_use]
    pub fn with_tool_concurrency(mut self, concurrency: usize) -> Self {
        self.tool_concurrency = concurrency.clamp(1, 64);
        self
    }

    #[must_use]
    pub fn with_tool(mut self, tool: impl tool::Tool + 'static) -> Self {
        self.register_tool(tool);
        self
    }

    /// Withdraw a tool before the next turn; absent names are ignored.
    pub fn remove_tool(&mut self, name: &str) {
        self.tools.remove(name);
    }

    /// Register or replace a tool before the next turn.
    pub fn register_tool(&mut self, tool: impl tool::Tool + 'static) {
        let tool: Arc<dyn tool::Tool> = Arc::new(tool);
        self.tools.register_arc(self.extension.as_ref().map_or_else(
            || tool.clone(),
            |extension| extension.wrap_tool(tool.clone()),
        ));
    }

    /// Extensions wrap execution after the scheduler's approval and confinement checks.
    #[must_use]
    pub fn with_runtime_extension(mut self, extension: Arc<dyn crate::RuntimeExtension>) -> Self {
        let tools = self.tools.iter().cloned().collect::<Vec<_>>();
        for tool in tools {
            self.tools.register_arc(extension.wrap_tool(tool));
        }
        self.extension = Some(extension);
        self
    }

    /// Seeds the kernel with a previously loaded session context.
    #[must_use]
    pub fn with_messages(mut self, mut messages: Vec<Message>) -> Self {
        if let Some(state) = ExecutionState::restore(&mut messages) {
            *self
                .execution
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
        } else {
            self.execution
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .seed_history(&messages);
        }
        self.task_queue = task_queue::TaskQueue::restore(&mut messages);
        self.goal_id = self.task_queue.as_ref().map(|q| q.goal_id.clone());
        self.parent_goal_id = self
            .task_queue
            .as_ref()
            .and_then(|q| q.parent_goal_id.clone());
        // Durable orchestration state that must outlive the context snapshot.
        self.child_results = child_result::restore(&mut messages);
        self.pending_question = user_input::restore(&mut messages);
        for message in &messages {
            if let Some(id) = &message.tool_call_id
                && let Ok(result) = serde_json::from_str::<tool::ToolResult>(&message.content)
                && let Ok(mut store) = self.result_reader.0.write()
            {
                store.insert(id.clone(), result.raw_output);
            }
        }
        self.messages = messages;
        self
    }

    #[must_use]
    pub fn fork_with_messages(&self, mut messages: Vec<Message>) -> Self {
        // Worker context belongs to a child goal, never the controller queue.
        task_queue::TaskQueue::restore(&mut messages);
        let result_reader = tool::ResultReader::default();
        let mut tools = self.tools.clone();
        tools.remove("subagent");
        tools.remove("subagent_fork");
        tools.remove("spawn_agent");
        tools.register(result_reader.clone());
        let mut worker = Self {
            extension: None,
            guard_model_requests: 0,
            continuation: crate::TurnState::default(),
            activity: Arc::default(),
            turn_input: crate::continuation::TurnInput::default(),
            stop_guard: None,
            provider: Arc::clone(&self.provider),
            retry_policy: self.retry_policy,
            permission_profiles: self.permission_profiles.clone(),
            child_models: self.child_models.clone(),
            context_pool: self.context_pool,
            tools,
            approval: Arc::clone(&self.approval),
            messages: Vec::new(),
            budget: self.budget,
            tool_concurrency: self.tool_concurrency,
            raw_turn_messages: Vec::new(),
            compression_dirty: false,
            result_reader,
            task_queue: None,
            goal_id: None,
            parent_goal_id: None,
            child_host: None,
            child_run: None,
            child_budget: None,
            child_concurrency: self.child_concurrency,
            child_results: std::collections::BTreeMap::new(),
            pending_question: None,
            receipts_dirty: false,
            subagent_config: SubagentConfig::default(),
            subagent_depth: 0,
            subagent_pool: None,
            subagent_tools: vec!["subagent".into(), "subagent_fork".into()],
            agent_templates: Vec::new(),
            subagent_manager: None,
            repeat_calls: crate::loop_hygiene::RepeatCallChain::default(),
            execution: Arc::new(std::sync::Mutex::new(ExecutionState::fresh())),
            execution_root: self.execution_root.clone(),
            progress_clock: Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
        }
        .with_messages(messages);
        worker.parent_goal_id.clone_from(&self.goal_id);
        worker
    }
}
