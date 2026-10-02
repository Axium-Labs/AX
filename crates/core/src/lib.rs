//! The provider-agnostic agent runtime kernel.

mod budget;
pub mod child;
pub mod child_policy;
pub use child::{ChildCheckpoint, ChildHost, ChildOutcome, ChildRun, PreparedChild};
pub use child_policy::ChildPolicy;
mod context;
pub mod execution;
mod scheduler;
pub mod subagent;
pub use execution::{ExecutionState, NoProgressDetector};
pub use subagent::{AgentTemplate, SpawnOptions, SubagentConfig, SubagentManager, SubagentResult};
pub mod task_queue;
pub use budget::{ContextBudget, ContextDemand, ContextPoolPolicy, ExecutionBudget};
pub use context::select_context;
pub use task_queue::{GoalTurn, QueueState};

use std::{collections::VecDeque, sync::Arc};

use async_trait::async_trait;
use model::{FunctionSpec, Message, ModelError, ModelProvider, ModelRequest, ToolSpec};
use serde_json::Value;
use thiserror::Error;
use tokio::task::JoinSet;
use tool::{SafetyLevel, ToolError, ToolOutput, ToolPermission, ToolRegistry};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    SubagentStarted {
        id: String,
    },
    SubagentProgress {
        id: String,
        phase: String,
    },
    SubagentCompleted {
        id: String,
    },
    SubagentFailed {
        id: String,
        error: String,
    },
    SubagentCancelled {
        id: String,
    },
    TurnStarted,
    ModelStarted {
        provider: String,
        model: String,
    },
    ContentDelta {
        delta: String,
    },
    /// Streaming reasoning / chain-of-thought, shown ahead of the answer.
    ThinkingDelta {
        delta: String,
    },
    ToolStarted {
        id: String,
        name: String,
        detail: String,
        input: Value,
    },
    ToolFinished {
        id: String,
        name: String,
        success: bool,
        diagnostics: Vec<tool::FetchError>,
        result: tool::ToolResult,
    },
    TurnFinished,
    ContextCompressed {
        removed_messages: usize,
        estimated_tokens_before: usize,
        estimated_tokens_after: usize,
        tokens_freed: usize,
        compression_ratio: f64,
        cleanup_tier: &'static str,
        semantic_called: bool,
        tool_outputs_reduced: usize,
        tool_outputs_removed: usize,
        recent_raw_tokens: usize,
    },
}

#[derive(Clone, Debug)]
pub struct CompressionResult {
    pub summary: String,
    pub removed_messages: usize,
    pub retained_messages: usize,
    pub estimated_tokens_before: usize,
    pub estimated_tokens_after: usize,
    pub tool_outputs_reduced: usize,
    pub tool_outputs_removed: usize,
    pub semantic_called: bool,
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("goal lifecycle error: {0}")]
    GoalMismatch(String),
    #[error("global execution blocker: {0}")]
    GlobalBlocked(String),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("invalid tool arguments for {tool}: {source}")]
    InvalidToolArguments {
        tool: String,
        source: serde_json::Error,
    },
    #[error("agent exceeded the maximum of {0} model steps")]
    StepLimit(usize),
    #[error("execution budget exhausted: {0}")]
    Budget(String),
    #[error("history persistence failed: {0}")]
    Persistence(String),
    #[error("agent worker failed: {0}")]
    WorkerJoin(String),
}

#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    async fn approve(&self, tool: &str, input: &Value, permission: ToolPermission) -> bool;
    /// Explicit Ask rules must not be bypassed by capability/session grants.
    fn capability_decision(
        &self,
        _capability: tool::Capability,
    ) -> Option<tool::PermissionDecision> {
        None
    }
    async fn ask(&self, _tool: &str, _input: &Value, _permission: ToolPermission) -> bool {
        false
    }
}

pub struct DenyDangerous;

#[async_trait]
impl ApprovalPolicy for DenyDangerous {
    async fn approve(&self, _tool: &str, _input: &Value, permission: ToolPermission) -> bool {
        permission.safety == SafetyLevel::Safe
    }
}

pub struct AllowAll;

#[async_trait]
impl ApprovalPolicy for AllowAll {
    async fn approve(&self, _tool: &str, _input: &Value, _permission: ToolPermission) -> bool {
        true
    }
}

pub struct AgentKernel {
    provider: Arc<dyn ModelProvider>,
    retry_policy: model::RetryPolicy,
    permission_profiles: Vec<tool::PermissionProfile>,
    child_models: std::collections::HashMap<String, Arc<dyn ModelProvider>>,
    context_pool: ContextPoolPolicy,
    tools: ToolRegistry,
    approval: Arc<dyn ApprovalPolicy>,
    messages: Vec<Message>,
    budget: ExecutionBudget,
    raw_turn_messages: Vec<Message>,
    compression_dirty: bool,
    tool_concurrency: usize,
    result_reader: tool::ResultReader,
    task_queue: Option<task_queue::TaskQueue>,
    goal_id: Option<String>,
    parent_goal_id: Option<String>,
    child_host: Option<Arc<dyn ChildHost>>,
    child_run: Option<ChildRun>,
    child_budget: Option<ExecutionBudget>,
    subagent_config: SubagentConfig,
    agent_templates: Vec<AgentTemplate>,
    subagent_manager: Option<Arc<SubagentManager>>,
    execution: Arc<std::sync::Mutex<ExecutionState>>,
    execution_root: Option<std::path::PathBuf>,
}

impl AgentKernel {
    #[must_use]
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        mut tools: ToolRegistry,
        approval: Arc<dyn ApprovalPolicy>,
    ) -> Self {
        if !provider.capabilities().vision {
            tools.remove("view_image");
        }
        let result_reader = tool::ResultReader::default();
        tools.register(result_reader.clone());
        Self {
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
            subagent_config: SubagentConfig::default(),
            agent_templates: Vec::new(),
            subagent_manager: None,
            execution: Arc::new(std::sync::Mutex::new(ExecutionState::fresh())),
            execution_root: None,
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
        self.tools.register(tool);
        self
    }

    /// Register or replace a tool before the next turn.
    pub fn register_tool(&mut self, tool: impl tool::Tool + 'static) {
        self.tools.register(tool);
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
    pub fn fork_with_messages(&self, mut messages: Vec<Message>) -> Self {
        // Worker context belongs to a child goal, never the controller queue.
        task_queue::TaskQueue::restore(&mut messages);
        let result_reader = tool::ResultReader::default();
        let mut tools = self.tools.clone();
        tools.remove("subagent");
        tools.remove("spawn_agent");
        tools.register(result_reader.clone());
        let mut worker = Self {
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
            subagent_config: SubagentConfig::default(),
            agent_templates: Vec::new(),
            subagent_manager: None,
            execution: Arc::new(std::sync::Mutex::new(ExecutionState::fresh())),
            execution_root: self.execution_root.clone(),
        }
        .with_messages(messages);
        worker.parent_goal_id.clone_from(&self.goal_id);
        worker
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

    /// Applies the layered compression pipeline when effective context is under
    /// pressure. Called before every model request in the agent loop.
    ///
    /// # Errors
    ///
    /// Returns an error when the summarization model call fails or returns no text.
    pub async fn compress_if_needed<F>(
        &mut self,
        emit: F,
    ) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compress(false, emit).await
    }

    /// Runs the same pipeline more aggressively on explicit user request.
    /// Recent messages and persistent system context remain protected.
    ///
    /// # Errors
    ///
    /// Returns an error when the summarization model call fails or produces
    /// an invalid response.
    pub async fn compact_now<F>(&mut self, emit: F) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compress(true, emit).await
    }

    #[allow(clippy::too_many_lines)]
    async fn compress<F>(
        &mut self,
        force: bool,
        mut emit: F,
    ) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let _timer = tool::telemetry::Timer::new("context.compress");
        let before = self.estimated_context_tokens();
        let original = self.messages.clone();
        let budget = self.context_budget();
        if !force && !budget.needs_compaction(before, self.pending_request_growth(budget)) {
            return Ok(None);
        }
        let target = if force {
            budget
                .pressure_target()
                .saturating_sub(budget.pool.next_request_reserve)
        } else {
            budget.pressure_target()
        };
        let need_to_free = before.saturating_sub(target);
        if need_to_free == 0 && !force {
            return Ok(None);
        }
        let recent_start = recent_raw_start(&self.messages, budget.recent_raw_budget());
        let recent_raw_tokens = estimate_tokens(&self.messages[recent_start..]);
        let mut reduced = 0;
        let mut tier = "cleanup";

        // Largest old, reproducible outputs first. Never mutate the raw turn log.
        let mut candidates = (0..recent_start)
            .filter(|&i| {
                self.messages[i].role == model::Role::Tool && self.messages[i].parts.is_empty()
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|&i| {
            std::cmp::Reverse(estimate_tokens(std::slice::from_ref(&self.messages[i])))
        });
        for i in candidates {
            if self.estimated_context_tokens() <= target && !force {
                break;
            }
            let old = self.messages[i].content.clone();
            if let Some(short) = compact_tool_output(&old, budget.compacted_tool_result_chars()) {
                self.messages[i].content = short;
                reduced += 1;
            }
        }

        // Repeated reads and repeated failures: keep the latest occurrence.
        if self.estimated_context_tokens() > target || force {
            tier = "deduplicate";
            let mut seen = std::collections::HashSet::new();
            let call_keys = self
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .map(|call| {
                    (
                        call.id.clone(),
                        format!("{}:{}", call.function.name, call.function.arguments),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>();
            for i in (0..self.messages.len()).rev() {
                if self.messages[i].role != model::Role::Tool || !self.messages[i].parts.is_empty()
                {
                    continue;
                }
                let content = self.messages[i].content.clone();
                let key = self.messages[i]
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| call_keys.get(id))
                    .cloned()
                    .unwrap_or(content.clone());
                if !seen.insert(key) && i < recent_start && estimate_text_tokens(&content) > 6 {
                    self.messages[i].content = "[duplicate tool output]".into();
                    reduced += 1;
                }
            }
        }

        let mut semantic_called = false;
        let mut removed_messages = 0;
        let mut tool_outputs_removed = 0;
        if self.estimated_context_tokens() > target || (force && reduced == 0) {
            // Only complete older turns are eligible. Existing summaries stay verbatim.
            let split = (recent_start..self.messages.len())
                .find(|&i| self.messages[i].role == model::Role::User)
                .unwrap_or(0);
            let old = &self.messages[..split];
            let transcript = old
                .iter()
                .filter(|m| m.role != model::Role::System)
                .map(|m| {
                    format!(
                        "{:?}: {}{}",
                        m.role,
                        m.content,
                        if m.tool_calls.is_empty() {
                            String::new()
                        } else {
                            format!("\nTool calls: {:?}", m.tool_calls)
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if !transcript.is_empty() {
                tool_outputs_removed = old.iter().filter(|m| m.role == model::Role::Tool).count();
                let mut compression_messages = vec![Message::system(
                    "Compress the older conversation into the smallest state sufficient to continue the task correctly. Preserve information that may affect future decisions, such as important user constraints, decisions, unresolved problems, relevant failures, current progress, and necessary facts. Decide semantically what matters. Do not invent information or repeat information already preserved by an earlier summary. Return only JSON: {\"state\":[{\"type\":\"other\",\"content\":\"...\",\"importance\":0.8}]}. Choose each entry's type as appropriate, for example constraint, decision, goal, fact, progress, failure, error, next_action, or other. Include only entries that matter; no type is required. Importance must be between 0 and 1.",
                )];
                if let Some(previous) = old
                    .iter()
                    .find(|m| m.content.starts_with("[memory-summary]"))
                {
                    compression_messages.push(Message::system(format!("Already preserved session state, for reference only. Do not rewrite it:\n{}", previous.content)));
                }
                compression_messages.push(Message::user(transcript));
                let response = self
                    .provider
                    .complete(ModelRequest {
                        messages: compression_messages,
                        tools: Vec::new(),
                    })
                    .await
                    .map_err(|error| {
                        self.messages.clone_from(&original);
                        AgentError::Model(error)
                    })?;
                if response.content.trim().is_empty() {
                    self.messages = original;
                    return Err(AgentError::Model(ModelError::InvalidResponse(
                        "context summarizer returned empty text".into(),
                    )));
                }
                let Some(new_state) = parse_semantic_state(&response.content) else {
                    self.messages = original;
                    return Ok(None);
                };
                semantic_called = true;
                tier = "semantic";
                let mut retained = self.messages.split_off(split);
                let mut persistent = self
                    .messages
                    .drain(..)
                    .filter(|m| m.role == model::Role::System)
                    .collect::<Vec<_>>();
                removed_messages = split.saturating_sub(persistent.len());
                let mut state = persistent
                    .iter()
                    .find(|m| m.content.starts_with("[memory-summary]"))
                    .map_or_else(Vec::new, |m| parse_saved_summary(&m.content));
                for entry in new_state {
                    if let Some(existing) = state
                        .iter_mut()
                        .find(|saved| saved.content.eq_ignore_ascii_case(&entry.content))
                    {
                        if entry.importance > existing.importance {
                            *existing = entry;
                        }
                    } else {
                        state.push(entry);
                    }
                }
                let Some(summary) = fit_summary(&state, budget.session_summary_budget()) else {
                    self.messages = original;
                    return Ok(None);
                };
                persistent.retain(|m| !m.content.starts_with("[memory-summary]"));
                persistent.insert(0, Message::system(summary));
                persistent.append(&mut retained);
                self.messages = persistent;
            }
        }
        let after = self.estimated_context_tokens();
        if after >= before {
            self.messages = original;
            return Ok(None);
        }
        let summary = self
            .messages
            .iter()
            .find(|m| m.content.starts_with("[memory-summary]"))
            .map_or_else(String::new, |m| {
                m.content
                    .trim_start_matches("[memory-summary]\n")
                    .to_owned()
            });
        self.compression_dirty = true;
        let ratio_milli = u32::try_from(after.saturating_mul(1000) / before.max(1)).unwrap_or(1000);
        let compression_ratio = f64::from(ratio_milli) / 1000.0;
        eprintln!(
            "[context.compress] tokens_before={before} tokens_after={after} tokens_freed={} compression_ratio={:.3} cleanup_tier={tier} semantic_called={semantic_called} tool_outputs_reduced={reduced} tool_outputs_removed={tool_outputs_removed} recent_raw_tokens={recent_raw_tokens} need_to_free={need_to_free} hard_pressure={}",
            before - after,
            compression_ratio,
            before >= budget.hard_pressure_threshold()
        );
        emit(AgentEvent::ContextCompressed {
            removed_messages,
            estimated_tokens_before: before,
            estimated_tokens_after: after,
            tokens_freed: before - after,
            compression_ratio,
            cleanup_tier: tier,
            semantic_called,
            tool_outputs_reduced: reduced,
            tool_outputs_removed,
            recent_raw_tokens,
        });
        Ok(Some(CompressionResult {
            summary,
            removed_messages,
            retained_messages: self.messages.len(),
            estimated_tokens_before: before,
            estimated_tokens_after: after,
            tool_outputs_reduced: reduced,
            tool_outputs_removed,
            semantic_called,
        }))
    }

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
                if !queue_active {
                    (emit.lock().unwrap())(AgentEvent::ContentDelta { delta });
                }
            };
            let on_thinking = |delta: String| {
                (emit.lock().unwrap())(AgentEvent::ThinkingDelta { delta });
            };
            let model_timer = tool::telemetry::Timer::new("model.request");
            let mut request_history = self.messages.clone();
            if let Some(queue) = self
                .task_queue
                .as_ref()
                .filter(|q| q.state == QueueState::Summarizing)
            {
                request_history.push(queue.summary_context());
            }
            let request_messages = request_context(&request_history, self.context_budget())?;
            let child_summary = self.child_host.is_some()
                && self
                    .task_queue
                    .as_ref()
                    .is_some_and(|q| q.delegate && q.state == QueueState::Summarizing);
            let request = ModelRequest {
                messages: request_messages,
                tools: if child_summary {
                    vec![]
                } else {
                    tool_specs.clone()
                },
            };
            let started = std::time::Instant::now();
            let mut attempts = 0;
            let response = loop {
                attempts += 1;
                let emitted = std::sync::atomic::AtomicBool::new(false);
                let response = {
                    let mut delta = |text: String| {
                        if !text.is_empty() {
                            emitted.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        on_delta(text);
                    };
                    let mut thinking = |text: String| {
                        if !text.is_empty() {
                            emitted.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        on_thinking(text);
                    };
                    let request =
                        self.provider
                            .complete_stream(request.clone(), &mut delta, &mut thinking);
                    if attempts == 1 {
                        request.await
                    } else {
                        let remaining =
                            std::time::Duration::from_millis(self.retry_policy.time_budget_ms)
                                .saturating_sub(started.elapsed());
                        tokio::time::timeout(remaining, request)
                            .await
                            .unwrap_or_else(|_| {
                                Err(ModelError::Io(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "provider retry time budget exhausted",
                                )))
                            })
                    }
                };
                match response {
                    Ok(response) => break response,
                    Err(error) => {
                        // Replaying a partially streamed response would duplicate output.
                        if emitted.load(std::sync::atomic::Ordering::Relaxed) {
                            return Err(error.into());
                        }
                        let entropy = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .subsec_nanos();
                        let Some(delay) = self.retry_policy.delay(
                            &error,
                            attempts,
                            started.elapsed(),
                            u64::from(entropy),
                        ) else {
                            return Err(error.into());
                        };
                        tokio::time::sleep(delay).await;
                    }
                }
            };
            drop(model_timer);
            let content = response.content;
            let tool_calls = response.tool_calls;
            if child_summary && !tool_calls.is_empty() {
                return Err(AgentError::GlobalBlocked(
                    "all children are terminal; controller summary cannot execute more tools"
                        .into(),
                ));
            }
            let mut assistant = Message::assistant(content.clone(), tool_calls.clone());
            assistant.usage = response.usage.map(|reported| serde_json::json!({"provider": self.provider.name(), "model": self.provider.model_id(), "reported": reported}));
            self.messages.push(assistant.clone());
            self.raw_turn_messages.push(assistant);
            checkpoint(&self.raw_turn_messages)?;

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
                return Ok(content);
            }

            if self.budget.max_tool_calls != 0
                && tool_calls.len() > self.budget.max_tool_calls.saturating_sub(calls_used)
            {
                return Err(AgentError::Budget("tool call limit".into()));
            }
            calls_used = calls_used.saturating_add(tool_calls.len());
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
                    Err(
                        "task_queue must be called alone; no tools in this round were executed"
                            .into(),
                    )
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
                    let input =
                        serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
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
                continue;
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
                    continue;
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
                    &emit,
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
                &mut subagent_events,
                &emit,
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
        }
    }

    fn begin_goal<H>(
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
        match intent {
            GoalTurn::New | GoalTurn::Start { .. } => {
                let goal_id = if let GoalTurn::Start { goal_id } = intent {
                    goal_id
                } else {
                    task_queue::fresh_goal_id()
                };
                if goal_id.trim().is_empty()
                    || self
                        .task_queue
                        .as_ref()
                        .is_some_and(|q| q.goal_id == goal_id)
                {
                    return Err(AgentError::GoalMismatch("new goal ID must be nonempty and fresh; use explicit resume for the saved goal".into()));
                }
                let had_queue = self.task_queue.is_some();
                // A durable empty head prevents restoring an archived queue when
                // the replacement goal is a plain single task.
                let next = if had_queue {
                    let mut queue = task_queue::TaskQueue::new(input.into(), vec![]);
                    queue.goal_id.clone_from(&goal_id);
                    queue.parent_goal_id.clone_from(&self.parent_goal_id);
                    Some(queue)
                } else {
                    None
                };
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
                self.set_context(task_queue::PROGRESS_PREFIX, None);
                self.messages
                    .retain(|m| !m.content.starts_with("[ax-recovery]"));
                // Supersede, archive and replacement head reach durable storage in
                // one checkpoint, so an interrupted turn cannot restore a
                // half-applied supersede. Raw history is still never truncated.
                if let Some(queue) = &self.task_queue {
                    self.raw_turn_messages.push(queue.snapshot());
                }
                checkpoint(&self.raw_turn_messages)?;
                Ok(None)
            }
            GoalTurn::Resume { goal_id } | GoalTurn::Cancel { goal_id } => {
                let queue = self
                    .task_queue
                    .as_mut()
                    .filter(|q| q.goal_id == goal_id)
                    .ok_or_else(|| {
                        AgentError::GoalMismatch(format!("no saved queue for goal {goal_id}"))
                    })?;
                if !matches!(
                    queue.state,
                    QueueState::Active | QueueState::Summarizing | QueueState::Suspended
                ) {
                    return Ok(Some(
                        queue
                            .final_response
                            .clone()
                            .or_else(|| queue.stop_reason.clone())
                            .unwrap_or_default(),
                    ));
                }
                if cancel {
                    queue.stop(QueueState::Cancelled, "Task queue canceled by user.".into());
                } else if queue.state == QueueState::Suspended {
                    queue.state = QueueState::Active;
                    queue.stop_reason = None;
                    queue.advance();
                }
                self.goal_id = Some(goal_id);
                self.checkpoint_queue(checkpoint)?;
                Ok(None)
            }
        }
    }

    fn finish_goal_response<F, H>(
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

    #[must_use]
    pub fn goal_id(&self) -> Option<&str> {
        self.goal_id.as_deref()
    }
    #[must_use]
    pub fn parent_goal_id(&self) -> Option<&str> {
        self.parent_goal_id.as_deref()
    }

    /// Durable queue checkpoint uses the caller's existing message persistence path.
    fn checkpoint_queue<H>(&mut self, checkpoint: &mut H) -> Result<(), AgentError>
    where
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        if let Some(queue) = &self.task_queue {
            self.raw_turn_messages.push(queue.snapshot());
            checkpoint(&self.raw_turn_messages)?;
        }
        Ok(())
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

fn fetch_diagnostics(name: &str, result: &Result<ToolOutput, ToolError>) -> Vec<tool::FetchError> {
    match result {
        Err(ToolError::WebFetch(errors)) => errors.clone(),
        Ok(ToolOutput::Text(text)) if name == "web" => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|value| serde_json::from_value(value["errors"].clone()).ok())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn tool_activity(name: &str, input: &Value) -> String {
    let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or("");
    let detail = match name {
        "search" => format!("searching '{}' in {}", field("query"), field("path")),
        "filesystem" => format!("{} {}", field("operation"), field("path")),
        "patch" => format!("editing {}", field("path")),
        "shell" => format!("running {}", field("command").lines().next().unwrap_or("")),
        "web" => {
            // `queries`/`urls` drive batched calls; the singular keys remain as
            // legacy aliases and also name the unit in the description.
            let (list_key, unit) = if field("operation") == "search" {
                ("queries", "query")
            } else {
                ("urls", "url")
            };
            let list = input.get(list_key).and_then(Value::as_array);
            let count = list
                .map_or(0, Vec::len)
                .max(usize::from(!field(unit).is_empty()));
            let first = list
                .and_then(|items| items.first())
                .and_then(Value::as_str)
                .unwrap_or_else(|| field(unit));
            format!(
                "{} {count} {}: {first}",
                field("operation"),
                if count == 1 { unit } else { list_key }
            )
        }
        "mcp" => format!("{} {} {}", field("action"), field("server"), field("tool")),
        _ if name.starts_with("mcp__") => format!("calling {name}"),
        _ => format!("calling {name}"),
    };
    detail.chars().take(120).collect()
}

#[cfg(test)]
mod tool_activity_tests {
    use super::tool_activity;

    #[test]
    fn fetch_diagnostics_survive_both_partial_and_total_failure() {
        use super::{ToolError, ToolOutput, fetch_diagnostics};
        let value = serde_json::json!({
            "url": "https://example.test/full",
            "kind": "connect",
            "reason": "connection failed",
            "error": "outer: root",
            "source_chain": ["outer", "root"]
        });
        let diagnostic = serde_json::from_value(value.clone()).unwrap();
        let failed = Err(ToolError::WebFetch(vec![diagnostic]));
        let partial = Ok(ToolOutput::Text(
            serde_json::json!({"errors": [value]}).to_string(),
        ));
        for result in [failed, partial] {
            let diagnostics = fetch_diagnostics("web", &result);
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].source_chain, ["outer", "root"]);
            assert_eq!(diagnostics[0].url, "https://example.test/full");
        }
        let search = Ok(ToolOutput::Text(
            serde_json::json!({"errors": [{"query": "q", "error": "search failed"}]}).to_string(),
        ));
        assert!(fetch_diagnostics("web", &search).is_empty());
    }
    use serde_json::json;

    #[test]
    fn describes_read_search_edit_and_command() {
        assert!(
            tool_activity("search", &json!({"query":"needle","path":"src"})).contains("needle")
        );
        assert!(
            tool_activity(
                "filesystem",
                &json!({"operation":"read","path":"README.md"})
            )
            .contains("read README.md")
        );
        assert!(
            tool_activity("patch", &json!({"path":"src/main.rs"})).contains("editing src/main.rs")
        );
        assert!(
            tool_activity("shell", &json!({"command":"cargo test\nother"}))
                .contains("running cargo test")
        );
    }

    #[test]
    fn describes_batched_web_calls() {
        assert_eq!(
            tool_activity(
                "web",
                &json!({"operation":"search","queries":["rust 1.94","rust notes"]})
            ),
            "search 2 queries: rust 1.94"
        );
        assert_eq!(
            tool_activity("web", &json!({"operation":"fetch","url":"https://ax.test"})),
            "fetch 1 url: https://ax.test"
        );
        assert_eq!(
            tool_activity("web", &json!({"operation":"search","query":"legacy"})),
            "search 1 query: legacy"
        );
    }
}

fn recent_raw_start(messages: &[Message], budget: usize) -> usize {
    let mut start = messages.len();
    let mut used: usize = 0;
    while start > 0 {
        let cost = estimate_tokens(&messages[start - 1..start]);
        if used.saturating_add(cost) > budget {
            break;
        }
        used += cost;
        start -= 1;
    }
    start
}

/// Compact one oversized tool result into a bounded diagnostic slice of itself:
/// a few leading lines plus lines that look like diagnostics. The allowance is
/// supplied by the caller's `ContextBudget`, so this tier owns no private
/// character count.
fn compact_tool_output(output: &str, allowance_chars: usize) -> Option<String> {
    let per_line = (allowance_chars / 8).max(1);
    let retained_total = (allowance_chars * 3 / 4).max(per_line);
    if output.len() <= retained_total {
        return None;
    }
    let lines = output.lines().collect::<Vec<_>>();
    let mut kept: Vec<String> = Vec::new();
    for line in lines.iter().take(2) {
        kept.push(line.chars().take(per_line).collect());
    }
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if ([
            "error",
            "fail",
            "panic",
            "stack overflow",
            "passed",
            "exit=",
            "exit code",
            "warning:",
            "test result:",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || (line.contains(".rs:") && line.chars().any(|c| c.is_ascii_digit())))
            && !kept.iter().any(|saved| saved == line)
        {
            kept.push(line.chars().take(per_line).collect());
        }
        if kept.iter().map(String::len).sum::<usize>() > retained_total {
            break;
        }
    }
    let mut short = format!("[compressed tool output; {} original lines]\n", lines.len());
    for line in kept {
        short.push_str(&line);
        short.push('\n');
    }
    if estimate_text_tokens(&short) >= estimate_text_tokens(output) {
        None
    } else {
        Some(short)
    }
}

#[derive(Clone, Debug)]
struct StateEntry {
    kind: String,
    content: String,
    importance: f64,
}

fn parse_semantic_state(output: &str) -> Option<Vec<StateEntry>> {
    let value: Value = serde_json::from_str(output.trim()).ok()?;
    let entries = value.get("state")?.as_array()?;
    if entries.is_empty() {
        return None;
    }
    entries
        .iter()
        .map(|entry| {
            let kind = entry.get("type")?.as_str()?.trim();
            let content = entry.get("content")?.as_str()?.trim();
            let importance = entry.get("importance")?.as_f64()?;
            if kind.is_empty()
                || content.is_empty()
                || !importance.is_finite()
                || !(0.0..=1.0).contains(&importance)
            {
                return None;
            }
            Some(StateEntry {
                kind: kind.to_owned(),
                content: content.to_owned(),
                importance,
            })
        })
        .collect()
}

fn parse_saved_summary(summary: &str) -> Vec<StateEntry> {
    let content = summary
        .strip_prefix("[memory-summary]\n")
        .unwrap_or(summary);
    parse_semantic_state(content).unwrap_or_else(|| {
        vec![StateEntry {
            kind: "other".into(),
            content: content.to_owned(),
            importance: 0.8,
        }]
    })
}

fn serialize_state(entries: &[StateEntry]) -> String {
    let state = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "type": entry.kind, "content": entry.content, "importance": entry.importance,
            })
        })
        .collect::<Vec<_>>();
    format!("[memory-summary]\n{}", serde_json::json!({"state": state}))
}

fn fit_summary(entries: &[StateEntry], budget: usize) -> Option<String> {
    let mut ranked = (0..entries.len()).collect::<Vec<_>>();
    let score = |index: usize| {
        let entry = &entries[index];
        let recent = f64::from(u32::try_from(index).unwrap_or(u32::MAX))
            / f64::from(u32::try_from(entries.len().max(1)).unwrap_or(u32::MAX));
        let type_weight = match entry.kind.as_str() {
            "constraint" | "decision" | "goal" | "failure" | "error" => 0.015,
            "next_action" | "progress" => 0.008,
            _ => 0.0,
        };
        entry.importance + 0.03 * recent + type_weight
    };
    ranked.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then_with(|| b.cmp(&a)));
    let mut selected = Vec::new();
    for index in ranked {
        let mut trial = selected.clone();
        trial.push(index);
        trial.sort_unstable();
        let candidate = serialize_state(
            &trial
                .iter()
                .map(|&i| entries[i].clone())
                .collect::<Vec<_>>(),
        );
        if estimate_tokens(&[Message::system(candidate)]) <= budget {
            selected = trial;
        }
    }
    if selected.is_empty() {
        return None;
    }
    Some(serialize_state(
        &selected
            .into_iter()
            .map(|i| entries[i].clone())
            .collect::<Vec<_>>(),
    ))
}

/// Selects request context without changing the effective in-memory transcript.
/// Old turns are removed first; optional retrieved context follows only when
/// it fits the same budget used to decide whether to compact.
/// Which pool a message joins when a request is assembled. Classification is
/// purely by the runtime's own context marker, never by tool or model output.
enum RequestPool {
    /// Runtime bookkeeping that must never be re-sent.
    Drop,
    /// Transient progress/recovery state, always kept first.
    Progress,
    /// Retrieved memory, optional when space is tight.
    Memory,
    /// Eligible skill metadata.
    Skill,
    /// Conversation history.
    History,
}

fn request_pool(message: &Message) -> RequestPool {
    if message.role != model::Role::System {
        return RequestPool::History;
    }
    let content = message.content.as_str();
    if content.starts_with("[ax-changes]\n") || content.starts_with(execution::STATE_PREFIX) {
        return RequestPool::Drop;
    }
    if content.starts_with(execution::CONTEXT_PREFIX)
        || content.starts_with(task_queue::PROGRESS_PREFIX)
        || content.starts_with("[ax-task-summary]")
        || content.starts_with("[ax-recovery]")
    {
        return RequestPool::Progress;
    }
    if content.starts_with("[retrieved-memory]") {
        return RequestPool::Memory;
    }
    if content.starts_with("[ax-skill:") {
        return RequestPool::Skill;
    }
    RequestPool::History
}

fn request_context(
    messages: &[Message],
    budget: ContextBudget,
) -> Result<Vec<Message>, AgentError> {
    let mut history = Vec::new();
    let mut memory = Vec::new();
    let mut skills = Vec::new();
    let mut progress = Vec::new();
    for message in messages {
        match request_pool(message) {
            RequestPool::Drop => {}
            RequestPool::Progress => progress.push(message.clone()),
            RequestPool::Memory => memory.push(message.clone()),
            RequestPool::Skill => skills.push(message.clone()),
            RequestPool::History => {
                let mut projected = message.clone();
                if message.role == model::Role::Tool
                    && let Ok(result) = serde_json::from_str::<tool::ToolResult>(&message.content)
                {
                    projected.content = result.model_view(budget.tool_result_chars());
                }
                history.push(projected);
            }
        }
    }

    let latest_turn_len = history
        .iter()
        .rposition(|message| message.role == model::Role::User)
        .map_or(0, |index| {
            history[index..]
                .iter()
                .filter(|message| message.role != model::Role::System)
                .count()
        });
    let progress_tokens = estimate_tokens(&progress);
    let latest_start = history
        .iter()
        .rposition(|m| m.role == model::Role::User)
        .unwrap_or(history.len());
    let latest_tokens = estimate_tokens(&history[latest_start..]);
    let allocations = budget
        .allocate(&[
            ContextDemand {
                demand: progress_tokens,
                minimum: progress_tokens,
                maximum: budget.usable(),
            },
            ContextDemand {
                demand: estimate_tokens(&skills),
                minimum: 0,
                maximum: budget.usable(),
            },
            ContextDemand {
                demand: estimate_tokens(&memory),
                minimum: 0,
                maximum: budget.usable(),
            },
            ContextDemand {
                demand: estimate_tokens(&history),
                minimum: latest_tokens,
                maximum: budget.usable(),
            },
        ])
        .map_err(|e| AgentError::Budget(e.into()))?;
    history = select_context(&history, allocations[3], allocations[3]);
    if history
        .iter()
        .filter(|message| message.role != model::Role::System)
        .count()
        < latest_turn_len
    {
        return Err(AgentError::Budget(
            "latest user turn exceeds the input budget".into(),
        ));
    }
    let mut selected = progress;
    selected.extend(history);
    let mut remaining = budget.usable().saturating_sub(estimate_tokens(&selected));
    // Routing and retrieval put their highest-priority entries first. When
    // space is tight, optional memory gives way before selected skills.
    for message in skills.into_iter().chain(memory) {
        let cost = estimate_tokens(std::slice::from_ref(&message));
        if cost <= remaining {
            selected.push(message);
            remaining -= cost;
        }
    }
    let estimated = estimate_tokens(&selected);
    if estimated > budget.usable() {
        return Err(AgentError::Budget(format!(
            "request needs {estimated} tokens, allowed {}",
            budget.usable()
        )));
    }
    Ok(selected)
}

/// Estimated token cost of the JSON tool schemas sent with every model
/// request, so context budgeting accounts for it instead of assuming
/// tool schemas are free.
#[must_use]
pub fn estimate_tool_schema_tokens(tools: &ToolRegistry) -> usize {
    tools
        .iter()
        .map(|tool| {
            estimate_text_tokens(tool.name())
                + estimate_text_tokens(tool.description())
                + estimate_text_tokens(&scheduler::input_schema(tool.input_schema()).to_string())
        })
        .sum()
}

#[must_use]
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
            estimate_text_tokens(&message.content)
                + message
                    .parts
                    .iter()
                    .map(|part| match part {
                        model::ContentPart::Text { text } => estimate_text_tokens(text),
                        model::ContentPart::Image { .. } => 2048,
                    })
                    .sum::<usize>()
                + message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        estimate_text_tokens(&call.function.name)
                            + estimate_text_tokens(&call.function.arguments)
                    })
                    .sum::<usize>()
                + message
                    .tool_call_id
                    .as_ref()
                    .map_or(0, |id| estimate_text_tokens(id))
                + 4
        })
        .sum()
}

fn estimate_text_tokens(text: &str) -> usize {
    let (ascii, non_ascii) = text.chars().fold((0_usize, 0_usize), |counts, character| {
        if character.is_ascii() {
            (counts.0 + 1, counts.1)
        } else {
            (counts.0, counts.1 + 1)
        }
    });
    ascii.div_ceil(4) + non_ascii.saturating_mul(2)
}

#[derive(Clone, Debug)]
pub struct AgentTask {
    pub id: String,
    pub prompt: String,
    pub context: Vec<Message>,
}

#[derive(Debug)]
pub struct AgentTaskResult {
    pub id: String,
    pub result: Result<String, AgentError>,
}

#[derive(Clone, Debug)]
pub enum MultiAgentEvent {
    Started {
        id: String,
    },
    /// Boxed: `AgentEvent` is much larger than the lifecycle variants, and this
    /// channel carries one event per streaming delta.
    Runtime {
        id: String,
        event: Box<AgentEvent>,
    },
    Finished {
        id: String,
    },
}

pub struct AgentSupervisor {
    template: AgentKernel,
    max_concurrency: usize,
}

impl AgentSupervisor {
    #[must_use]
    pub fn new(template: AgentKernel, max_concurrency: usize) -> Self {
        Self {
            template,
            max_concurrency: max_concurrency.max(1),
        }
    }

    /// Runs independent agent contexts with bounded concurrency.
    ///
    /// # Errors
    ///
    /// Individual model/tool failures are returned inside each task result.
    /// This method returns a top-level error only when a worker task panics or is cancelled.
    pub async fn run_tasks(
        &self,
        tasks: Vec<AgentTask>,
        events: Option<tokio::sync::mpsc::UnboundedSender<MultiAgentEvent>>,
    ) -> Result<Vec<AgentTaskResult>, AgentError> {
        let mut pending = tasks.into_iter().collect::<VecDeque<_>>();
        let mut running = JoinSet::new();
        let mut results = Vec::with_capacity(pending.len());

        while !pending.is_empty() || !running.is_empty() {
            while running.len() < self.max_concurrency
                && let Some(task) = pending.pop_front()
            {
                let mut kernel = self.template.fork_with_messages(task.context);
                let sender = events.clone();
                running.spawn(async move {
                    if let Some(sender) = &sender {
                        let _ = sender.send(MultiAgentEvent::Started {
                            id: task.id.clone(),
                        });
                    }
                    let id = task.id;
                    let event_id = id.clone();
                    let result = kernel
                        .run_turn(task.prompt, |event| {
                            if let Some(sender) = &sender {
                                let _ = sender.send(MultiAgentEvent::Runtime {
                                    id: event_id.clone(),
                                    event: Box::new(event),
                                });
                            }
                        })
                        .await;
                    if let Some(sender) = &sender {
                        let _ = sender.send(MultiAgentEvent::Finished { id: id.clone() });
                    }
                    AgentTaskResult { id, result }
                });
            }

            if let Some(result) = running.join_next().await {
                results.push(result.map_err(|error| AgentError::WorkerJoin(error.to_string()))?);
            }
        }
        results.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use model::{FunctionCall, ModelResponse, ToolCall};
    use serde_json::json;

    use super::*;
    use tool::Tool;

    struct ScriptedProvider {
        model: String,
        responses: Mutex<VecDeque<ModelResponse>>,
    }

    #[async_trait]
    impl ModelProvider for ScriptedProvider {
        fn name(&self) -> &'static str {
            "test"
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            &self.model
        }

        fn context_window(&self) -> usize {
            6_000
        }

        fn max_output_tokens(&self) -> Option<usize> {
            Some(100)
        }

        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
            self.responses
                .lock()
                .expect("response mutex poisoned")
                .pop_front()
                .ok_or_else(|| ModelError::InvalidResponse("script exhausted".to_owned()))
        }
    }

    struct EchoTool;

    #[tokio::test]
    async fn failed_patch_recovers_with_local_read_patch_and_minimal_check() {
        let path = std::env::temp_dir().join(format!(
            "ax-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::write(&path, "old\nkeep\n").await.unwrap();
        let call = |id: &str, name: &str, input: Value| ModelResponse {
            usage: None,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: id.into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: input.to_string(),
                },
            }],
            finish_reason: None,
        };
        let provider = ScriptedProvider {
            model: "recovery".into(),
            responses: Mutex::new(VecDeque::from([
                call(
                    "bad",
                    "patch",
                    serde_json::json!({"path":path,"edits":[{"start_line":1,"delete_count":1,"expected_lines":["stale"],"new_text":"new\n"}]}),
                ),
                call(
                    "read",
                    "filesystem",
                    serde_json::json!({"operation":"read","path":path,"start_line":1,"end_line":2}),
                ),
                call(
                    "fix",
                    "patch",
                    serde_json::json!({"path":path,"edits":[{"start_line":1,"delete_count":1,"expected_lines":["old"],"new_text":"new\n"}]}),
                ),
                call(
                    "check",
                    "filesystem",
                    serde_json::json!({"operation":"read","path":path,"start_line":1,"end_line":1}),
                ),
                ModelResponse {
                    usage: None,
                    content: "done".into(),
                    tool_calls: vec![],
                    finish_reason: None,
                },
            ])),
        };
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(tool::PatchTool);
        tools.register(tool::FilesystemTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), tools, Arc::new(AllowAll))
            .with_execution_scope(path.parent().unwrap().to_owned());
        let mut finished = Vec::new();
        kernel
            .run_turn("repair this file", |event| {
                if let AgentEvent::ToolFinished {
                    id,
                    success,
                    result,
                    ..
                } = event
                {
                    finished.push((id, success, result));
                }
            })
            .await
            .unwrap();
        assert_eq!(
            finished
                .iter()
                .map(|(id, ok, _)| (id.as_str(), *ok))
                .collect::<Vec<_>>(),
            [
                ("bad", false),
                ("read", true),
                ("fix", true),
                ("check", true)
            ]
        );
        assert!(
            serde_json::to_string(&finished[0].2.diagnostics)
                .unwrap()
                .contains("patch_conflict")
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "new\nkeep\n"
        );
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn checkpoint_failure_stops_before_tool_execution_and_keeps_recoverable_history() {
        let provider = ScriptedProvider {
            model: "scripted".into(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                usage: None,
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "echo".into(),
                        arguments: "{}".into(),
                    },
                }],
                finish_reason: None,
            }])),
        };
        let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
        registry.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));
        let mut saved = Vec::new();
        let mut failed_once = false;
        let result = kernel
            .run_turn_checkpointed(
                "test",
                |_| {},
                |messages| {
                    if messages.iter().any(|m| !m.tool_calls.is_empty()) && !failed_once {
                        failed_once = true;
                        return Err(AgentError::Persistence("disk failure".into()));
                    }
                    saved = messages.to_vec();
                    Ok(())
                },
            )
            .await;
        assert!(matches!(result, Err(AgentError::Persistence(_))));
        let conversation: Vec<_> = saved
            .iter()
            .filter(|m| m.role != model::Role::System)
            .collect();
        assert_eq!(conversation.len(), 3);
        assert_eq!(conversation[0].role, model::Role::User);
        assert!(conversation[2].content.contains("Execution interrupted"));
    }

    #[tokio::test]
    async fn each_model_and_tool_message_is_checkpointed_in_order() {
        let provider = ScriptedProvider {
            model: "scripted".into(),
            responses: Mutex::new(VecDeque::from([
                ModelResponse {
                    usage: None,
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "echo".into(),
                            arguments: "{}".into(),
                        },
                    }],
                    finish_reason: None,
                },
                ModelResponse {
                    usage: None,
                    content: "done".into(),
                    tool_calls: vec![],
                    finish_reason: None,
                },
            ])),
        };
        let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
        registry.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));
        let mut lengths = Vec::new();
        kernel
            .run_turn_checkpointed(
                "test",
                |_| {},
                |messages| {
                    lengths.push(messages.len());
                    Ok(())
                },
            )
            .await
            .unwrap();
        // Goal admission checkpoints the empty history before the first input.
        assert_eq!(lengths, vec![0, 2, 3, 5, 6, 6]);
    }

    struct EchoProvider;

    #[async_trait]
    impl ModelProvider for EchoProvider {
        fn name(&self) -> &'static str {
            "echo"
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &'static str {
            "echo-model"
        }

        fn context_window(&self) -> usize {
            6_000
        }

        fn max_output_tokens(&self) -> Option<usize> {
            Some(100)
        }

        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse {
                usage: None,
                content: request
                    .messages
                    .last()
                    .map_or_else(String::new, |message| message.content.clone()),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            })
        }
    }

    #[async_trait]
    #[allow(clippy::unnecessary_literal_bound)]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echo input"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn capability(&self, _input: &Value) -> tool::Capability {
            tool::Capability::Process
        }
        fn safety(&self, _input: &Value) -> SafetyLevel {
            SafetyLevel::Safe
        }

        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            Ok(input.to_string())
        }
    }

    #[tokio::test]
    async fn loops_through_tool_result_to_final_answer() {
        let provider = ScriptedProvider {
            model: "scripted".to_owned(),
            responses: Mutex::new(VecDeque::from([
                ModelResponse {
                    usage: None,
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".to_owned(),
                        kind: "function".to_owned(),
                        function: FunctionCall {
                            name: "echo".to_owned(),
                            arguments: r#"{"value":"hello"}"#.to_owned(),
                        },
                    }],
                    finish_reason: Some("tool_calls".to_owned()),
                },
                ModelResponse {
                    usage: None,
                    content: "done".to_owned(),
                    tool_calls: Vec::new(),
                    finish_reason: Some("stop".to_owned()),
                },
            ])),
        };
        let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
        registry.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));

        let answer = kernel
            .run_turn("use echo", |_| {})
            .await
            .expect("agent turn should succeed");

        assert_eq!(answer, "done");
        assert_eq!(
            kernel
                .messages()
                .iter()
                .filter(|m| m.role != model::Role::System)
                .count(),
            4
        );
        assert_eq!(kernel.messages()[2].role, model::Role::Tool);
    }

    #[tokio::test]
    async fn compresses_old_context_without_truncating_recent_messages() {
        // Trigger comes from ContextBudget pressure alone; no configured
        // percentage participates in the decision.
        let provider = ScriptedProvider {
            model: "scripted".to_owned(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
            usage: None,
                content: r#"{"state":[{"type":"goal","content":"goal and decisions preserved","importance":0.9}]}"#.to_owned(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            }])),
        };
        let messages = (0..6)
            .map(|index| Message::user(format!("{index}:{}", "x".repeat(3500))))
            .collect();
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(DenyDangerous),
        )
        .with_messages(messages);

        kernel.configure_context_pool(ContextPoolPolicy {
            recent_raw_maximum: 1000,
            ..Default::default()
        });
        let result = kernel
            .compress_if_needed(|_| {})
            .await
            .expect("compression should succeed")
            .expect("context should exceed threshold");

        assert_eq!(result.removed_messages, 5);
        assert_eq!(kernel.messages().len(), 2);
        assert!(kernel.messages()[0].content.contains("goal and decisions"));
        assert!(kernel.messages()[1].content.starts_with("5:"));
    }

    #[test]
    fn request_context_fits_history_memory_skills_and_large_tool_schema() {
        let budget = ContextBudget::new(6_000, Some(500), 1_200);
        let mut messages = (0..30)
            .map(|index| Message::user(format!("old {index}:{}", "x".repeat(400))))
            .collect::<Vec<_>>();
        messages.push(Message::system(format!(
            "[retrieved-memory]\n{}",
            "m".repeat(500)
        )));
        messages.push(Message::system(format!(
            "[ax-skill:first]\n{}",
            "s".repeat(2_000)
        )));
        messages.push(Message::system(format!(
            "[ax-skill:second]\n{}",
            "s".repeat(2_000)
        )));
        messages.push(Message::user("current request"));

        let selected = request_context(&messages, budget).expect("request should fit");
        assert!(estimate_tokens(&selected) <= budget.usable());
        assert!(
            selected
                .iter()
                .any(|message| message.content == "current request")
        );
        assert!(
            selected
                .iter()
                .filter(|message| message.content.starts_with("old "))
                .count()
                < 30
        );
        assert_eq!(
            messages.len(),
            34,
            "selection must not mutate source history"
        );
    }

    #[test]
    fn tiny_context_rejects_an_oversized_latest_turn() {
        let budget = ContextBudget::new(1_000, Some(100), 700);
        let messages = vec![Message::user("x".repeat(2_000))];
        assert!(matches!(
            request_context(&messages, budget),
            Err(AgentError::Budget(_))
        ));
    }

    #[tokio::test]
    async fn compacted_context_stays_within_final_request_budget() {
        let provider = ScriptedProvider {
            model: "scripted".to_owned(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                usage: None,
                content:
                    r#"{"state":[{"type":"progress","content":"short summary","importance":0.8}]}"#
                        .to_owned(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            }])),
        };
        let messages = (0..30)
            .map(|index| Message::user(format!("{index}:{}", "x".repeat(600))))
            .collect();
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(DenyDangerous),
        )
        .with_messages(messages);
        assert!(kernel.compress_if_needed(|_| {}).await.unwrap().is_some());
        kernel.push_context(Message::system(format!(
            "[retrieved-memory]\n{}",
            "m".repeat(500)
        )));
        kernel.push_context(Message::system(format!(
            "[ax-skill:test]\n{}",
            "s".repeat(2_000)
        )));
        kernel.push_context(Message::user("new question"));
        let selected = request_context(kernel.messages(), kernel.context_budget()).unwrap();
        assert!(estimate_tokens(&selected) <= kernel.context_budget().usable());
        assert!(
            selected
                .iter()
                .any(|message| message.content == "new question")
        );
    }

    struct RecordingProvider {
        replies: Mutex<VecDeque<ModelResponse>>,
        request_tokens: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl ModelProvider for RecordingProvider {
        fn name(&self) -> &'static str {
            "recording"
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &'static str {
            "recording"
        }
        fn context_window(&self) -> usize {
            6_000
        }
        fn max_output_tokens(&self) -> Option<usize> {
            Some(100)
        }
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            if request
                .messages
                .first()
                .is_some_and(|m| m.content.starts_with("Compress the older conversation"))
            {
                return Ok(ModelResponse {
                    usage: None,
                    content: r#"{"state":[{"type":"goal","content":"finish","importance":0.9}]}"#
                        .into(),
                    tool_calls: vec![],
                    finish_reason: None,
                });
            }
            self.request_tokens
                .lock()
                .unwrap()
                .push(estimate_tokens(&request.messages));
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ModelError::InvalidResponse("script exhausted".into()))
        }
    }

    struct LargeTestTool;
    #[async_trait]
    impl Tool for LargeTestTool {
        #[allow(clippy::unnecessary_literal_bound)]
        fn name(&self) -> &'static str {
            "test"
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn description(&self) -> &str {
            "Run tests"
        }
        fn input_schema(&self) -> Value {
            json!({"type":"object"})
        }
        fn capability(&self, _: &Value) -> tool::Capability {
            tool::Capability::Process
        }
        fn safety(&self, _: &Value) -> SafetyLevel {
            SafetyLevel::Safe
        }
        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            if input["large"] == true {
                Ok(format!(
                    "cargo test\n{}\ntest result: FAILED. 243 passed; 1 failed\nparser::tests::nested\nsrc/parser.rs:281 stack overflow\nexit=101",
                    "progress line\n".repeat(2000)
                ))
            } else {
                Ok("quick check passed".into())
            }
        }
    }

    fn test_call(id: &str, large: bool) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "test".into(),
                arguments: format!("{{\"large\":{large}}}"),
            },
        }
    }

    #[tokio::test]
    async fn compresses_between_tool_calls_and_preserves_raw_turn() {
        let provider = Arc::new(RecordingProvider {
            replies: Mutex::new(VecDeque::from([
                ModelResponse {
                    usage: None,
                    content: String::new(),
                    tool_calls: vec![test_call("a", true)],
                    finish_reason: None,
                },
                ModelResponse {
                    usage: None,
                    content: String::new(),
                    tool_calls: vec![test_call("b", false)],
                    finish_reason: None,
                },
                ModelResponse {
                    usage: None,
                    content: "done".into(),
                    tool_calls: vec![],
                    finish_reason: None,
                },
            ])),
            request_tokens: Mutex::new(vec![]),
        });
        let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
        registry.register(LargeTestTool);
        let mut kernel = AgentKernel::new(provider.clone(), registry, Arc::new(DenyDangerous));
        let mut events = Vec::new();
        assert_eq!(
            kernel
                .run_turn("run tests", |e| events.push(e))
                .await
                .unwrap(),
            "done"
        );
        let sizes = provider.request_tokens.lock().unwrap().clone();
        assert_eq!(sizes.len(), 3);
        assert!(
            sizes[1] < 1000,
            "second request should see reduced test output: {sizes:?}"
        );
        assert!(
            kernel
                .messages()
                .iter()
                .any(|message| message.role == model::Role::Tool && message.content.len() < 4000)
        );
        assert!(
            kernel
                .messages()
                .iter()
                .any(|m| m.content.contains("243 passed; 1 failed"))
        );
        let raw = kernel.take_turn_messages();
        assert!(raw.iter().any(|m| m.content.len() > 20_000));
    }

    #[tokio::test]
    async fn semantic_state_keeps_early_constraints_and_failures_without_summary_recursion() {
        let provider = ScriptedProvider { model: "scripted".into(), responses: Mutex::new(VecDeque::from([
            ModelResponse { usage: None, content: r#"{"state":[{"type":"constraint","content":"Do not change the public API","importance":1.0},{"type":"failure","content":"Approach A failed because of a parser stack overflow","importance":0.95}]}"#.into(), tool_calls: vec![], finish_reason: None }
        ])) };
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(DenyDangerous),
        )
        .with_messages(vec![
            Message::system("[memory-summary]\nDECISIONS: keep SQLite"),
            Message::user(format!(
                "Do not change the public API\n{}",
                "task details ".repeat(1500)
            )),
            Message::assistant(
                "Approach A failed because of a parser stack overflow",
                vec![],
            ),
            Message::user("continue"),
        ]);
        let result = kernel.compact_now(|_| {}).await.unwrap().unwrap();
        assert!(result.semantic_called);
        assert!(result.summary.contains("Do not change the public API"));
        assert!(
            result
                .summary
                .contains("Approach A failed because of a parser stack overflow")
        );
        assert!(result.summary.contains("DECISIONS: keep SQLite"));
        assert_eq!(kernel.messages().last().unwrap().content, "continue");
    }

    #[tokio::test]
    async fn repeated_file_reads_collapse_older_tool_result() {
        let provider = Arc::new(RecordingProvider {
            replies: Mutex::new(VecDeque::new()),
            request_tokens: Mutex::new(vec![]),
        });
        let read = |id: &str| ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{\"path\":\"src/lib.rs\"}".into(),
            },
        };
        let mut kernel = AgentKernel::new(
            provider,
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(DenyDangerous),
        )
        .with_messages(vec![
            Message::user("inspect file"),
            Message::assistant("", vec![read("first")]),
            Message::tool("first", "old file body \n".repeat(500)),
            Message::assistant("", vec![read("second")]),
            Message::tool("second", "new file body \n".repeat(500)),
            Message::user("current task"),
        ]);
        kernel.configure_context_pool(ContextPoolPolicy {
            recent_raw_maximum: 100,
            ..Default::default()
        });
        kernel.compact_now(|_| {}).await.unwrap();
        assert!(
            kernel
                .messages()
                .iter()
                .any(|m| m.content.contains("duplicate tool output"))
        );
    }

    #[test]
    fn structured_summary_uses_importance_over_keywords_and_type() {
        let entries = vec![
            StateEntry {
                kind: "other".into(),
                content: "All deliverables use British English".into(),
                importance: 0.95,
            },
            StateEntry {
                kind: "error".into(),
                content: "The word error appeared in a harmless example".into(),
                importance: 0.05,
            },
        ];
        let high_only = serialize_state(&entries[..1]);
        let budget = estimate_tokens(&[Message::system(high_only)]) + 2;
        let fitted = fit_summary(&entries, budget).unwrap();
        let parsed = parse_saved_summary(&fitted);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].content, "All deliverables use British English");
        assert!(estimate_tokens(&[Message::system(fitted)]) <= budget);
    }

    #[tokio::test]
    async fn semantic_model_can_preserve_constraint_without_keyword() {
        let provider = ScriptedProvider { model: "scripted".into(), responses: Mutex::new(VecDeque::from([
            ModelResponse { usage: None, content: r#"{"state":[{"type":"constraint","content":"All deliverables use British English","importance":0.98}]}"#.into(), tool_calls: vec![], finish_reason: None }
        ])) };
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(DenyDangerous),
        )
        .with_messages(vec![
            Message::user(format!(
                "All deliverables use British English\n{}",
                "background ".repeat(2000)
            )),
            Message::assistant("noted", vec![]),
            Message::user("continue"),
        ]);
        let result = kernel.compact_now(|_| {}).await.unwrap().unwrap();
        assert!(
            result
                .summary
                .contains("All deliverables use British English")
        );
        let selected = request_context(kernel.messages(), kernel.context_budget()).unwrap();
        assert!(estimate_tokens(&selected) <= kernel.context_budget().usable());
    }

    #[tokio::test]
    async fn malformed_structured_output_keeps_original_context() {
        let provider = ScriptedProvider {
            model: "scripted".into(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                usage: None,
                content: "{broken JSON".into(),
                tool_calls: vec![],
                finish_reason: None,
            }])),
        };
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(DenyDangerous),
        )
        .with_messages(vec![
            Message::user("long context ".repeat(2000)),
            Message::assistant("old answer", vec![]),
            Message::user("continue"),
        ]);
        let original = serde_json::to_string(kernel.messages()).unwrap();
        assert!(kernel.compact_now(|_| {}).await.unwrap().is_none());
        assert_eq!(serde_json::to_string(kernel.messages()).unwrap(), original);
        assert!(!kernel.take_compression_dirty());
    }

    #[tokio::test]
    async fn supervisor_runs_independent_tasks() {
        let template = AgentKernel::new(
            Arc::new(EchoProvider),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(DenyDangerous),
        );
        let supervisor = AgentSupervisor::new(template, 2);
        let results = supervisor
            .run_tasks(
                vec![
                    AgentTask {
                        id: "b".to_owned(),
                        prompt: "second".to_owned(),
                        context: Vec::new(),
                    },
                    AgentTask {
                        id: "a".to_owned(),
                        prompt: "first".to_owned(),
                        context: Vec::new(),
                    },
                ],
                None,
            )
            .await
            .expect("supervisor should finish");

        assert_eq!(results[0].id, "a");
        assert_eq!(
            results[0]
                .result
                .as_deref()
                .expect("first agent should succeed"),
            "first"
        );
        assert_eq!(
            results[1]
                .result
                .as_deref()
                .expect("second agent should succeed"),
            "second"
        );
    }
    #[tokio::test]
    async fn tool_budget_stops_before_execution_and_keeps_valid_transcript() {
        let provider = ScriptedProvider {
            model: "test".into(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                usage: None,
                content: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "pending-1".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "echo".into(),
                            arguments: "{}".into(),
                        },
                    },
                    ToolCall {
                        id: "pending-2".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "echo".into(),
                            arguments: "{}".into(),
                        },
                    },
                ],
                finish_reason: None,
            }])),
        };
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), tools, Arc::new(AllowAll))
            .with_execution_budget(ExecutionBudget {
                max_tool_calls: 1,
                ..ExecutionBudget::default()
            });
        assert!(matches!(
            kernel.run_turn("test", |_| {}).await,
            Err(AgentError::Budget(_))
        ));
        assert_eq!(
            kernel.messages().last().unwrap().tool_call_id.as_deref(),
            Some("pending-2")
        );
        assert!(
            kernel
                .messages()
                .last()
                .unwrap()
                .content
                .contains("interrupted")
        );
    }

    struct PendingProvider;
    #[async_trait]
    impl ModelProvider for PendingProvider {
        fn name(&self) -> &'static str {
            "pending"
        }
        fn model_id(&self) -> &'static str {
            "pending"
        }
        fn context_window(&self) -> usize {
            1000
        }
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn turn_timeout_is_enforced_while_model_is_waiting() {
        let mut kernel = AgentKernel::new(
            Arc::new(PendingProvider),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(AllowAll),
        )
        .with_execution_budget(ExecutionBudget {
            turn_timeout_secs: 1,
            ..ExecutionBudget::default()
        });
        assert!(matches!(
            kernel.run_turn("wait", |_| {}).await,
            Err(AgentError::Budget(_))
        ));
        assert_eq!(kernel.messages()[0].content, "wait");
    }
}

#[cfg(test)]
mod task_queue_tests;

#[cfg(test)]
mod execution_tests;

#[cfg(test)]
mod retry_integration_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Flaky {
        calls: AtomicUsize,
        code: u16,
        partial: bool,
        hang_retry: bool,
    }
    #[async_trait]
    impl ModelProvider for Flaky {
        fn name(&self) -> &'static str {
            "retry-test"
        }
        fn model_id(&self) -> &'static str {
            "retry-test"
        }
        fn context_window(&self) -> usize {
            32000
        }
        async fn complete(&self, _: ModelRequest) -> Result<model::ModelResponse, ModelError> {
            unreachable!()
        }
        async fn complete_stream(
            &self,
            _: ModelRequest,
            delta: &mut (dyn FnMut(String) + Send),
            _: &mut (dyn FnMut(String) + Send),
        ) -> Result<model::ModelResponse, ModelError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 || self.partial {
                if self.partial {
                    delta("partial".into());
                }
                return Err(ModelError::HttpResponse {
                    status: self.code,
                    message: String::new(),
                    retry_after: Some(std::time::Duration::ZERO),
                });
            }
            if self.hang_retry {
                std::future::pending::<()>().await;
            }
            Ok(model::ModelResponse {
                content: "done".into(),
                tool_calls: vec![],
                usage: None,
                finish_reason: None,
            })
        }
    }
    #[tokio::test]
    async fn retries_transient_errors_only_and_never_replays_partial_stream() {
        for (code, partial, expected) in [
            (503, false, 2),
            (429, false, 2),
            (400, false, 1),
            (401, false, 1),
            (403, false, 1),
            (503, true, 1),
        ] {
            let provider = Arc::new(Flaky {
                calls: AtomicUsize::new(0),
                code,
                partial,
                hang_retry: false,
            });
            let mut kernel = AgentKernel::new(
                provider.clone(),
                ToolRegistry::with_mode(tool::SandboxMode::Off),
                Arc::new(AllowAll),
            );
            let result = kernel.run_turn("request", |_| {}).await;
            assert_eq!(provider.calls.load(Ordering::SeqCst), expected);
            assert_eq!(result.is_ok(), expected == 2);
        }
    }
    #[tokio::test]
    async fn retry_timeout_respects_remaining_time_budget() {
        let provider = Arc::new(Flaky {
            calls: AtomicUsize::new(0),
            code: 503,
            partial: false,
            hang_retry: true,
        });
        let mut kernel = AgentKernel::new(
            provider.clone(),
            ToolRegistry::with_mode(tool::SandboxMode::Off),
            Arc::new(AllowAll),
        );
        kernel.configure_retry(model::RetryPolicy {
            max_attempts: 4,
            time_budget_ms: 10,
            base_delay_ms: 0,
            max_delay_ms: 0,
        });
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                kernel.run_turn("request", |_| {})
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }
}
