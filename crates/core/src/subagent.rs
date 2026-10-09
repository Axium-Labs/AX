//! Optional delegation adapter. All execution uses `ChildHost` and `AgentSupervisor`.
use crate::{
    AgentError, AgentEvent, AgentKernel, AgentSupervisor, ChildHost,
    child_result::{ChildResult, ChildStatus},
};
use async_trait::async_trait;
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::{Notify, Semaphore, mpsc};
use tool::{Capability, ResourceAccess, SafetyLevel, Tool, ToolError};

/// Metadata for an enabled named agent. Bodies remain on disk until invocation.
#[derive(Clone, Debug)]
pub struct AgentTemplate {
    pub name: String,
    pub description: String,
    pub instructions: std::path::PathBuf,
    pub tools: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SubagentConfig {
    /// Maximum live children sharing the concurrency pool (deepseek-harness's
    /// `maxActiveSubagents`).
    pub max_concurrent: usize,
    /// Absolute delegation-depth limit. `0` disables delegation; `1` (default)
    /// permits direct children, and larger values permit nested delegation.
    pub max_depth: usize,
}
impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            max_depth: 1,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
    pub context: Option<String>,
    pub tools: Option<Vec<String>>,
    pub timeout_secs: u64,
    pub policy: crate::ChildPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubagentResult {
    pub status: String,
    pub summary: String,
    pub artifacts: Vec<String>,
    pub error: Option<String>,
}
impl SubagentResult {
    fn failed(status: &str, error: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            summary: String::new(),
            artifacts: vec![],
            error: Some(error.into()),
        }
    }
}
struct Pending {
    input: String,
    options: SpawnOptions,
    cancelled: AtomicBool,
    notify: Notify,
    result: tokio::sync::Mutex<Option<SubagentResult>>,
    done: Notify,
    finished: AtomicBool,
    consumed: AtomicBool,
}

/// Turn-scoped admission and cancellation only; contains no model execution loop.
pub struct SubagentManager {
    controller: AgentKernel,
    templates: Vec<AgentTemplate>,
    parent_messages: Vec<model::Message>,
    host: Arc<dyn ChildHost>,
    pool: Arc<SubagentPool>,
    accepting: AtomicBool,
    pending: Mutex<std::collections::HashMap<String, Arc<Pending>>>,
    events: mpsc::UnboundedSender<AgentEvent>,
}
/// Shared by all descendants of one turn. Ancestors waiting for children still
/// occupy slots, so nested admission must reject exhaustion instead of queueing.
pub(crate) struct SubagentPool {
    slots: Semaphore,
    next: AtomicU64,
    admitted: AtomicU64,
}
impl SubagentManager {
    fn child_tool_allowed(&self, name: &str) -> bool {
        if ["subagent", "subagent_fork"].contains(&name) {
            self.controller.subagent_depth.saturating_add(1)
                < self.controller.subagent_config.max_depth
                && self
                    .controller
                    .subagent_tools
                    .iter()
                    .any(|tool| tool == name)
        } else {
            self.controller.has_tool(name)
        }
    }
    fn event(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) fn continuation_counts(&self) -> (usize, usize) {
        let entries = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let running = entries
            .values()
            .filter(|p| !p.finished.load(Ordering::Acquire))
            .count();
        let ready = entries
            .values()
            .filter(|p| p.finished.load(Ordering::Acquire) && !p.consumed.load(Ordering::Acquire))
            .count();
        (running, ready)
    }

    pub(crate) async fn collect_unconsumed(&self) -> Vec<(String, SubagentResult)> {
        let ids = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, p)| !p.consumed.load(Ordering::Acquire))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut results = Vec::new();
        for id in ids {
            let result = self.wait_agent(&id).await;
            results.push((id, result));
        }
        results
    }

    /// Admit an isolated task. Tools can only narrow the parent's capabilities.
    ///
    /// # Errors
    /// Returns an error for disabled delegation, invalid input or exhausted admission.
    pub fn spawn_agent(
        self: &Arc<Self>,
        task: &str,
        options: SpawnOptions,
    ) -> Result<String, AgentError> {
        if !self.accepting.load(Ordering::Acquire) {
            return Err(ToolError::Execution("subagents are disabled for this turn".into()).into());
        }
        if self.controller.subagent_depth >= self.controller.subagent_config.max_depth {
            return Err(ToolError::Execution("subagent maximum depth reached".into()).into());
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| AgentError::WorkerJoin(error.to_string()))?;
        if task.trim().is_empty() {
            return Err(ToolError::InvalidInput("task is empty".into()).into());
        }
        if let Some(names) = &options.tools {
            for name in names {
                if !self.child_tool_allowed(name) {
                    return Err(ToolError::PermissionDenied(name.clone()).into());
                }
            }
        }
        options
            .policy
            .inherited_context(&self.parent_messages)
            .map_err(|e| ToolError::InvalidInput(e.into()))?;
        for name in options
            .policy
            .selected_tools
            .iter()
            .chain(&options.policy.selected_mcp)
        {
            if !self.child_tool_allowed(name) {
                return Err(ToolError::PermissionDenied(name.clone()).into());
            }
        }
        if options.policy.model == crate::child_policy::ModelInheritance::Override
            && !options
                .policy
                .model_override
                .as_ref()
                .is_some_and(|name| self.controller.child_models.contains_key(name))
        {
            return Err(ToolError::InvalidInput(
                "model override must be registered by the parent".into(),
            )
            .into());
        }
        let id = format!(
            "subagent-{}",
            self.pool.next.fetch_add(1, Ordering::Relaxed)
        );
        let input = options.context.as_ref().map_or_else(
            || task.to_owned(),
            |context| format!("{task}\n\nContext:\n{context}"),
        );
        let mut entries = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.accepting.load(Ordering::Acquire) {
            return Err(ToolError::Execution("subagents are disabled for this turn".into()).into());
        }
        if self
            .pool
            .admitted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < 64).then_some(count + 1)
            })
            .is_err()
        {
            return Err(ToolError::Execution(
                "subagent admission limit reached for this turn".into(),
            )
            .into());
        }
        entries.insert(
            id.clone(),
            Arc::new(Pending {
                input,
                options,
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
                done: Notify::new(),
                finished: AtomicBool::new(false),
                consumed: AtomicBool::new(false),
                result: tokio::sync::Mutex::new(None),
            }),
        );
        drop(entries);
        let manager = Arc::clone(self);
        let worker_id = id.clone();
        runtime.spawn(async move {
            manager.run_agent(&worker_id).await;
        });
        Ok(id)
    }

    #[must_use]
    pub fn cancel_agent(&self, id: &str) -> bool {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned();
        if let Some(pending) = pending {
            if pending.finished.load(Ordering::Acquire) {
                return false;
            }
            pending.cancelled.store(true, Ordering::Release);
            pending.notify.notify_one();
            true
        } else {
            false
        }
    }

    fn shutdown(&self) {
        self.accepting.store(false, Ordering::Release);
        for pending in self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
        {
            pending.cancelled.store(true, Ordering::Release);
            pending.notify.notify_one();
        }
    }

    /// Wait for the final receipt only; child history stays in its own session.
    pub async fn wait_agent(&self, id: &str) -> SubagentResult {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(pending) = pending else {
            return SubagentResult::failed("failed", "unknown subagent");
        };
        loop {
            let done = pending.done.notified();
            tokio::pin!(done);
            done.as_mut().enable();
            if let Some(result) = pending.result.lock().await.as_ref() {
                pending.consumed.store(true, Ordering::Release);
                return result.clone();
            }
            done.await;
        }
    }

    // Admission, binding and durable receipt form one child lifecycle.
    #[allow(clippy::too_many_lines)]
    async fn execute_child(
        &self,
        id: &str,
        pending: &Pending,
    ) -> Result<SubagentResult, AgentError> {
        let _slot = if self.controller.subagent_depth == 0 {
            self.pool
                .slots
                .acquire()
                .await
                .map_err(|e| AgentError::WorkerJoin(e.to_string()))?
        } else {
            self.pool.slots.try_acquire().map_err(|_| {
                ToolError::Execution(
                    "subagent concurrency limit reached; waiting ancestors count toward the limit"
                        .into(),
                )
            })?
        };
        // The timeout bounds child execution only. Waiting behind the
        // concurrency limit is queueing, and must not consume the child's own
        // execution time or report a timeout for work that never started.
        if pending.options.timeout_secs == 0 {
            return self.run_child(id, pending).await;
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(pending.options.timeout_secs),
            self.run_child(id, pending),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(SubagentResult::failed("failed", "subagent timed out")),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn run_child(&self, id: &str, pending: &Pending) -> Result<SubagentResult, AgentError> {
        let child = self
            .host
            .prepare_with_policy(&self.controller, &pending.input, &pending.options.policy)
            .await?;
        let mut guard = ChildGuard {
            child,
            finished: false,
        };
        let child = &mut guard.child;
        let policy = &pending.options.policy;
        child.kernel.approval = Arc::clone(&self.controller.approval);
        child
            .kernel
            .permission_profiles
            .clone_from(&self.controller.permission_profiles);
        if child.kernel.permission_profiles.is_empty() {
            child
                .kernel
                .permission_profiles
                .push(tool::PermissionProfile::default());
        }
        if policy.permissions == crate::child_policy::PermissionInheritance::Custom {
            child
                .kernel
                .constrain_permissions(policy.custom_permissions.clone());
        }
        child.kernel.provider = policy
            .model_override
            .as_ref()
            .filter(|_| policy.model == crate::child_policy::ModelInheritance::Override)
            .and_then(|name| self.controller.child_models.get(name))
            .cloned()
            .unwrap_or_else(|| Arc::clone(&self.controller.provider));
        // deepseek-harness's delegation contract: the child's permission scope
        // was fixed at start and can never be widened from inside.
        child.kernel.messages.push(model::Message::system(
            "[ax-delegation]\nYou are a delegated subagent: your permission scope was fixed when you were started and cannot be widened from inside this session - operations that require approval are rejected automatically. When the task needs access beyond that scope, do not retry the denied operation; state the limitation in your reply so the delegating agent can handle it.".to_owned(),
        ));
        let inherited = policy
            .inherited_context(&self.parent_messages)
            .map_err(|e| ToolError::InvalidInput(e.into()))?;
        // Parent history is context, never part of the child's durable raw-turn log.
        if !inherited.is_empty() {
            child.kernel.messages.insert(
                0,
                model::Message::system(format!(
                    "[ax-parent-context]\nReadonly context from parent:\n{}",
                    serde_json::to_string(&inherited)
                        .map_err(|e| ToolError::InvalidInput(e.to_string()))?
                )),
            );
        }
        let existing = child
            .kernel
            .tools
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for name in existing {
            let class = child
                .kernel
                .tools
                .get(&name)
                .map_or(tool::InheritanceClass::Tools, |t| t.inheritance_class());
            let keep = match class {
                tool::InheritanceClass::Memory => {
                    policy.memory != crate::child_policy::MemoryInheritance::None
                }
                tool::InheritanceClass::Skills => {
                    policy.skills != crate::child_policy::Selection::None
                }
                tool::InheritanceClass::Mcp => match policy.mcp {
                    crate::child_policy::Selection::None => false,
                    crate::child_policy::Selection::Selected => policy.selected_mcp.contains(&name),
                    crate::child_policy::Selection::Inherit => true,
                },
                tool::InheritanceClass::Tools => match policy.tools {
                    crate::child_policy::Selection::None => false,
                    crate::child_policy::Selection::Selected => {
                        policy.selected_tools.contains(&name)
                    }
                    crate::child_policy::Selection::Inherit => true,
                },
            };
            if !self.controller.has_tool(&name)
                || !keep
                || pending
                    .options
                    .tools
                    .as_ref()
                    .is_some_and(|names| !names.contains(&name))
                || name == "subagent"
                || name == "subagent_fork"
                || name == "spawn_agent"
            {
                child.kernel.tools.remove(&name);
            }
        }
        // Sharing state is opt-in and mediated by a tool's typed binding hook.
        for parent_tool in self.controller.tools.iter() {
            if pending
                .options
                .tools
                .as_ref()
                .is_some_and(|names| !names.iter().any(|n| n == parent_tool.name()))
            {
                continue;
            }
            if parent_tool.inheritance_class() == tool::InheritanceClass::Skills
                && policy.skills != crate::child_policy::Selection::None
            {
                let selected = (policy.skills == crate::child_policy::Selection::Selected)
                    .then_some(policy.selected_skills.as_slice());
                if let Some(tool) = parent_tool.fork_skills(selected) {
                    child.kernel.tools.register_arc(tool);
                } else {
                    return Err(ToolError::InvalidInput(
                        "skill tool cannot support inheritance".into(),
                    )
                    .into());
                }
            }
            if parent_tool.inheritance_class() == tool::InheritanceClass::Memory {
                let shared = match policy.memory {
                    crate::child_policy::MemoryInheritance::ParentReadonly => {
                        parent_tool.fork_memory(&pending.input, true)
                    }
                    crate::child_policy::MemoryInheritance::SharedProject => {
                        parent_tool.fork_memory(&pending.input, false)
                    }
                    _ => None,
                };
                if let Some(tool) = shared {
                    child.kernel.tools.register_arc(tool);
                } else if matches!(
                    policy.memory,
                    crate::child_policy::MemoryInheritance::ParentReadonly
                        | crate::child_policy::MemoryInheritance::SharedProject
                ) {
                    return Err(ToolError::InvalidInput(
                        "memory tool cannot support requested inheritance".into(),
                    )
                    .into());
                }
            }
            if parent_tool.inheritance_class() == tool::InheritanceClass::Mcp {
                let selected = match policy.mcp {
                    crate::child_policy::Selection::None => false,
                    crate::child_policy::Selection::Selected => {
                        policy.selected_mcp.contains(&parent_tool.name().to_owned())
                    }
                    crate::child_policy::Selection::Inherit => true,
                };
                // Parent-scoped remote resources are explicit shared state.
                if selected {
                    if policy.workspace != crate::child_policy::WorkspaceInheritance::Shared {
                        return Err(ToolError::InvalidInput(
                            "parent MCP inheritance requires shared workspace".into(),
                        )
                        .into());
                    }
                    child.kernel.tools.register_arc(parent_tool.clone());
                }
            }
        }
        // Rebind delegation to this child, never copy a tool bound to its parent.
        child.kernel.subagent_config = self.controller.subagent_config;
        child.kernel.subagent_depth = self.controller.subagent_depth + 1;
        child.kernel.subagent_pool = Some(Arc::clone(&self.pool));
        child.kernel.subagent_tools = self
            .controller
            .subagent_tools
            .iter()
            .filter(|name| {
                let selected = match policy.tools {
                    crate::child_policy::Selection::None => false,
                    crate::child_policy::Selection::Selected => {
                        policy.selected_tools.contains(name)
                    }
                    crate::child_policy::Selection::Inherit => true,
                };
                selected
                    && pending
                        .options
                        .tools
                        .as_ref()
                        .is_none_or(|tools| tools.contains(name))
            })
            .cloned()
            .collect();
        child.kernel.agent_templates.clone_from(&self.templates);
        child.kernel.child_host = Some(
            self.host
                .fork_for_child(child.kernel.child_run.as_ref().unwrap_or(&child.run))
                .unwrap_or_else(|| Arc::clone(&self.host)),
        );
        self.event(AgentEvent::SubagentStarted { id: id.into() });
        let mut result: ChildResult = if let Some(outcome) = child.terminal.take() {
            outcome
        } else {
            let output = AgentSupervisor::run_child(
                &mut child.kernel,
                &pending.input,
                Box::new(|event| {
                    if matches!(
                        &event,
                        AgentEvent::SubagentStarted { .. }
                            | AgentEvent::SubagentProgress { .. }
                            | AgentEvent::SubagentCompleted { .. }
                            | AgentEvent::SubagentFailed { .. }
                            | AgentEvent::SubagentCancelled { .. }
                    ) {
                        self.event(event);
                        return;
                    }
                    let phase = match event {
                        AgentEvent::ModelStarted { .. } => Some("model"),
                        AgentEvent::ToolStarted { .. } => Some("tool"),
                        _ => None,
                    };
                    if let Some(phase) = phase {
                        self.event(AgentEvent::SubagentProgress {
                            id: id.into(),
                            phase: phase.into(),
                        });
                    }
                }),
                Box::new(|messages| child.checkpoint.save(messages)),
            )
            .await;
            match output {
                Ok(output) => crate::child::terminal_result(child.kernel.messages())
                    .unwrap_or_else(|| {
                        let mut result = ChildResult::new(id, id, ChildStatus::Completed);
                        result.summary = output;
                        result
                    }),
                Err(error) => ChildResult::failed(id, id, ChildStatus::Failed, error.to_string()),
            }
        };
        child.checkpoint.finish(&mut result)?;
        guard.finished = true;
        Ok::<_, AgentError>(if result.status.success() {
            SubagentResult {
                status: "completed".into(),
                summary: result.summary,
                artifacts: guard
                    .child
                    .run
                    .state_dir
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .into_iter()
                    .collect(),
                error: None,
            }
        } else {
            SubagentResult::failed("failed", result.summary)
        })
    }

    async fn run_agent(&self, id: &str) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
            .expect("admitted subagent");
        let cancelled = async {
            loop {
                let notified = pending.notify.notified();
                if pending.cancelled.load(Ordering::Acquire) {
                    break;
                }
                notified.await;
            }
        };
        let execution = self.execute_child(id, &pending);
        let result = tokio::select! {
            biased;
            () = cancelled => SubagentResult::failed("cancelled", "cancelled"),
            result = std::panic::AssertUnwindSafe(execution).catch_unwind() => match result {
                Ok(result) => result.unwrap_or_else(|error| SubagentResult::failed("failed", error.to_string())),
                Err(_) => SubagentResult::failed("failed", "subagent worker panicked"),
            },
        };
        self.event(match result.status.as_str() {
            "completed" => AgentEvent::SubagentCompleted { id: id.into() },
            "cancelled" => AgentEvent::SubagentCancelled { id: id.into() },
            _ => AgentEvent::SubagentFailed {
                id: id.into(),
                error: result.error.clone().unwrap_or_default(),
            },
        });
        *pending.result.lock().await = Some(result);
        pending.finished.store(true, Ordering::Release);
        pending.done.notify_waiters();
    }
}

// Dropping a tool round (parent timeout/cancel) also closes the durable child receipt.
struct ChildGuard {
    child: crate::PreparedChild,
    finished: bool,
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(manager) = &self.child.kernel.subagent_manager {
            manager.shutdown();
        }
        if !self.finished {
            let mut result = ChildResult::new(
                self.child.run.session_id.clone(),
                self.child.run.goal_id.clone(),
                ChildStatus::Cancelled,
            );
            result.failure_reason = Some("subagent cancelled or timed out".into());
            result.summary = "subagent cancelled or timed out".into();
            let _ = self.child.checkpoint.finish(&mut result);
        }
    }
}

struct CancelOnDrop {
    manager: Arc<SubagentManager>,
    id: String,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let _ = self.manager.cancel_agent(&self.id);
    }
}

/// One delegation implementation, instantiated twice like deepseek-harness's
/// `tool-subagent`: `subagent` spawns a fresh-context child, `subagent_fork`
/// delegates to a child seeded with the parent's completed turns.
struct SubagentTool {
    manager: Arc<SubagentManager>,
    fork: bool,
}
impl SubagentTool {
    fn spawn(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager,
            fork: false,
        }
    }
    fn fork(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager,
            fork: true,
        }
    }
}
#[async_trait]
impl Tool for SubagentTool {
    fn execution_boundary(&self) -> tool::ExecutionBoundary {
        tool::ExecutionBoundary::RuntimeOwned
    }
    fn name(&self) -> &'static str {
        if self.fork {
            "subagent_fork"
        } else {
            "subagent"
        }
    }
    fn description(&self) -> &'static str {
        if self.fork {
            "Delegate a task to a subagent that inherits this conversation: a child agent seeded with all completed turns so far (it does not see the current in-flight turn). Use this when the subtask builds on this conversation's context - a follow-up analysis, a review, a continuation - without consuming this conversation's context for the work itself. It shares the parent workspace and provider. You receive its result, not its intermediate steps. Further delegation is limited by the configured depth and shared concurrency budget."
        } else {
            "Delegate a self-contained task to a subagent (a separate agent that works in its own context) to offload focused, independent work - research, a scoped implementation, an analysis - so it does not consume this conversation's context. The subagent returns its result, not its intermediate steps. Child tools may only be narrowed. Further delegation is limited by the configured depth and shared concurrency budget."
        }
    }
    fn input_schema(&self) -> Value {
        if self.fork {
            return json!({"type":"object","properties":{"description":{"type":"string","description":"A short (3-5 word) description of the delegated task, for display."},"prompt":{"type":"string","description":"The task for the subagent. It already sees this conversation's completed turns, so build on them freely and state only what is new."}},"required":["prompt"],"additionalProperties":false});
        }
        let mut schema = json!({"type":"object","properties":{"task":{"type":"string","description":"The complete, self-contained task for the subagent. It does not share this conversation's context, so include everything it needs."},"context":{"type":"string"},"tools":{"type":"array","items":{"type":"string"}},"policy":{"type":"object","description":"ChildPolicy inheritance contract. Default: no parent context, isolated memory/workspace, no skills/MCP. Parent permission ceiling always applies."}},"required":["task"],"additionalProperties":false});
        schema["properties"]["policy"] = crate::ChildPolicy::schema();
        if !self.manager.templates.is_empty() {
            schema["properties"]["agent"] = json!({"type":"string","enum":self.manager.templates.iter().map(|a| &a.name).collect::<Vec<_>>(),"description":self.manager.templates.iter().map(|a| format!("{}: {}", a.name, a.description)).collect::<Vec<_>>().join("; ")});
        }
        schema
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn capability(&self, _: &Value) -> Capability {
        Capability::Process
    }
    fn resources(&self, _: &Value) -> Vec<ResourceAccess> {
        // Child effect tools acquire process-wide resource leases themselves.
        // Holding a parent lease while awaiting a shared child would deadlock.
        vec![]
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            #[serde(alias = "prompt")]
            task: String,
            description: Option<String>,
            agent: Option<String>,
            context: Option<String>,
            tools: Option<Vec<String>>,
            #[serde(default)]
            policy: crate::ChildPolicy,
        }
        let mut input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if self.fork {
            // The fork contract is fixed: completed-turn context, parent
            // workspace, parent model. No model selection, no narrowing.
            let label = input.description.unwrap_or_default();
            let prompt = input.task;
            input.task = if label.trim().is_empty() {
                prompt
            } else {
                format!("{label}: {prompt}")
            };
            input.context = None;
            input.tools = None;
            input.agent = None;
            input.policy = crate::ChildPolicy {
                context: crate::child_policy::ContextInheritance::CompletedTurns,
                workspace: crate::child_policy::WorkspaceInheritance::Shared,
                ..crate::ChildPolicy::default()
            };
        }
        if let Some(name) = &input.agent {
            let template = self
                .manager
                .templates
                .iter()
                .find(|a| &a.name == name)
                .ok_or_else(|| {
                    ToolError::InvalidInput(format!("Unknown or disabled agent: {name}"))
                })?;
            let instructions = tokio::fs::read_to_string(&template.instructions)
                .await
                .map_err(|e| {
                    ToolError::Execution(format!("Cannot load agent instructions: {e}"))
                })?;
            input.context = Some(format!(
                "{instructions}\n{}",
                input.context.unwrap_or_default()
            ));
            input.tools = match (&template.tools, input.tools) {
                (Some(allowed), Some(requested)) => {
                    if requested.iter().any(|tool| !allowed.contains(tool)) {
                        return Err(ToolError::InvalidInput(
                            "Agent tools may only be narrowed".into(),
                        ));
                    }
                    Some(requested)
                }
                (Some(allowed), None) => Some(allowed.clone()),
                (None, requested) => requested,
            };
        }
        let result = match self.manager.spawn_agent(
            &input.task,
            SpawnOptions {
                context: input.context,
                tools: input.tools,
                policy: input.policy,
                timeout_secs: self
                    .manager
                    .controller
                    .child_execution_budget()
                    .turn_timeout_secs,
            },
        ) {
            Ok(id) => {
                let _cancellation = CancelOnDrop {
                    manager: Arc::clone(&self.manager),
                    id: id.clone(),
                };
                self.manager.wait_agent(&id).await
            }
            Err(error) => SubagentResult::failed("failed", error.to_string()),
        };
        let output =
            serde_json::to_string(&result).map_err(|e| ToolError::Execution(e.to_string()))?;
        if result.status == "completed" {
            Ok(output)
        } else {
            Err(ToolError::Execution(output))
        }
    }
}

impl AgentKernel {
    /// Submit through the current turn's optional delegation primitive.
    ///
    /// # Errors
    /// Returns an error if delegation is disabled or admission is rejected.
    pub fn spawn_agent(&self, task: &str, options: SpawnOptions) -> Result<String, AgentError> {
        self.subagent_manager
            .as_ref()
            .ok_or_else(|| ToolError::Execution("subagents are not enabled for this turn".into()))?
            .spawn_agent(task, options)
    }
    pub async fn wait_agent(&self, id: &str) -> SubagentResult {
        match &self.subagent_manager {
            Some(manager) => manager.wait_agent(id).await,
            None => SubagentResult::failed("failed", "subagents are not enabled for this turn"),
        }
    }
    #[must_use]
    pub fn cancel_agent(&self, id: &str) -> bool {
        self.subagent_manager
            .as_ref()
            .is_some_and(|manager| manager.cancel_agent(id))
    }
    pub fn configure_subagents(&mut self, config: SubagentConfig) {
        self.subagent_config = config;
        if let Some(manager) = self.subagent_manager.take() {
            manager.shutdown();
        }
        self.tools.remove("subagent");
        self.tools.remove("subagent_fork");
    }
    pub fn configure_agent_templates(&mut self, templates: Vec<AgentTemplate>) {
        self.agent_templates = templates;
    }
    /// The optional runtime primitive handle, initialized only on enabled turns.
    #[must_use]
    pub fn subagent_manager(&self) -> Option<Arc<SubagentManager>> {
        self.subagent_manager.clone()
    }
    /// Prepare delegation without performing a model call or provisioning a
    /// child. A depth budget of zero is the only off switch.
    pub fn prepare_subagents(&mut self) -> Option<mpsc::UnboundedReceiver<AgentEvent>> {
        if self.subagent_depth >= self.subagent_config.max_depth
            || self.subagent_tools.is_empty()
            || (self.child_run.is_some() && self.subagent_pool.is_none())
        {
            return None;
        }
        let host = self.child_host.clone()?;
        if let Some(previous) = self.subagent_manager.take() {
            previous.shutdown();
        }
        let mut controller = self.fork_with_messages(vec![]);
        controller.tools.remove("subagent");
        controller.tools.remove("subagent_fork");
        controller.child_budget = self.child_budget;
        controller.subagent_config = self.subagent_config;
        controller.subagent_depth = self.subagent_depth;
        controller.subagent_tools.clone_from(&self.subagent_tools);
        let pool = self.subagent_pool.clone().unwrap_or_else(|| {
            Arc::new(SubagentPool {
                slots: Semaphore::new(self.subagent_config.max_concurrent.clamp(1, 64)),
                next: AtomicU64::new(1),
                admitted: AtomicU64::new(0),
            })
        });
        let (events, receiver) = mpsc::unbounded_channel();
        let manager = Arc::new(SubagentManager {
            controller,
            templates: self.agent_templates.clone(),
            parent_messages: self.messages.clone(),
            host,
            pool,
            accepting: AtomicBool::new(true),
            pending: Mutex::new(std::collections::HashMap::new()),
            events,
        });
        self.subagent_manager = Some(Arc::clone(&manager));
        if self.subagent_tools.iter().any(|name| name == "subagent") {
            self.tools
                .register(SubagentTool::spawn(Arc::clone(&manager)));
        }
        if self
            .subagent_tools
            .iter()
            .any(|name| name == "subagent_fork")
        {
            self.tools.register(SubagentTool::fork(manager));
        }
        Some(receiver)
    }
}

pub(crate) async fn forward_events<T, F: FnMut(AgentEvent) + Send>(
    future: impl std::future::Future<Output = T>,
    receiver: &mut Option<mpsc::UnboundedReceiver<AgentEvent>>,
    emit: &Mutex<F>,
) -> T {
    let Some(receiver) = receiver else {
        return future.await;
    };
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            Some(event) = receiver.recv() => (emit.lock().unwrap_or_else(std::sync::PoisonError::into_inner))(event),
            result = &mut future => {
                while let Ok(event) = receiver.try_recv() { (emit.lock().unwrap_or_else(std::sync::PoisonError::into_inner))(event); }
                return result;
            }
        }
    }
}

#[cfg(test)]
#[path = "subagent_tests.rs"]
mod tests;
