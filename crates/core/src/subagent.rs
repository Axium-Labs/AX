//! Optional delegation adapter. All execution uses `ChildHost` and `AgentSupervisor`.
use crate::{AgentError, AgentEvent, AgentKernel, AgentSupervisor, ChildHost, ChildOutcome};
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
    pub enabled: bool,
    pub max_concurrent: usize,
    pub max_depth: usize,
}
impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_concurrent: 3,
            max_depth: 1,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
    pub context: Option<String>,
    pub tools: Option<Vec<String>>,
    pub timeout_secs: u64,
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
}

/// Turn-scoped admission and cancellation only; contains no model execution loop.
pub struct SubagentManager {
    controller: AgentKernel,
    templates: Vec<AgentTemplate>,
    host: Arc<dyn ChildHost>,
    slots: Semaphore,
    next: AtomicU64,
    accepting: AtomicBool,
    pending: Mutex<std::collections::HashMap<String, Arc<Pending>>>,
    events: mpsc::UnboundedSender<AgentEvent>,
}
impl SubagentManager {
    fn event(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    /// Admit an isolated task. Unknown or recursive tools are rejected, never added.
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
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| AgentError::WorkerJoin(error.to_string()))?;
        if task.trim().is_empty() {
            return Err(ToolError::InvalidInput("task is empty".into()).into());
        }
        if let Some(names) = &options.tools {
            for name in names {
                if name == "subagent" || name == "spawn_agent" || !self.controller.has_tool(name) {
                    return Err(ToolError::PermissionDenied(name.clone()).into());
                }
            }
        }
        let id = format!("subagent-{}", self.next.fetch_add(1, Ordering::Relaxed));
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
        if entries.len() >= 64 {
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
                return result.clone();
            }
            done.await;
        }
    }

    async fn execute_child(
        &self,
        id: &str,
        pending: &Pending,
    ) -> Result<SubagentResult, AgentError> {
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|e| AgentError::WorkerJoin(e.to_string()))?;
        let child = self
            .host
            .prepare(&self.controller, &pending.input, None)
            .await?;
        let mut guard = ChildGuard {
            child,
            finished: false,
        };
        let child = &mut guard.child;
        child.kernel.approval = Arc::clone(&self.controller.approval);
        child.kernel.provider = Arc::clone(&self.controller.provider);
        child.kernel.tools.remove("subagent");
        child.kernel.tools.remove("spawn_agent");
        if let Some(names) = &pending.options.tools {
            let existing = child
                .kernel
                .tools
                .names()
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for name in existing {
                if !names.contains(&name) {
                    child.kernel.tools.remove(&name);
                }
            }
        }
        self.event(AgentEvent::SubagentStarted { id: id.into() });
        let result = if let Some(outcome) = child.terminal.take() {
            outcome
        } else {
            let output = AgentSupervisor::run_child(
                &mut child.kernel,
                &pending.input,
                Box::new(|event| {
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
                Ok(output) => crate::child::terminal_outcome(child.kernel.messages()).unwrap_or(
                    ChildOutcome {
                        success: true,
                        output,
                    },
                ),
                Err(error) => ChildOutcome {
                    success: false,
                    output: error.to_string(),
                },
            }
        };
        child.checkpoint.finish(&result)?;
        guard.finished = true;
        Ok::<_, AgentError>(if result.success {
            SubagentResult {
                status: "completed".into(),
                summary: result.output,
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
            SubagentResult::failed("failed", result.output)
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
            () = async {
                if pending.options.timeout_secs == 0 { std::future::pending::<()>().await; }
                else { tokio::time::sleep(std::time::Duration::from_secs(pending.options.timeout_secs)).await; }
            } => SubagentResult::failed("failed", "subagent timed out"),
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
        if !self.finished {
            let _ = self.child.checkpoint.finish(&ChildOutcome {
                success: false,
                output: "subagent cancelled or timed out".into(),
            });
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

struct SubagentTool(Arc<SubagentManager>);
#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &'static str {
        "subagent"
    }
    fn description(&self) -> &'static str {
        "Delegate an independent task with only necessary context. Returns its final result. Child tools may only be narrowed; children cannot delegate."
    }
    fn input_schema(&self) -> Value {
        let mut schema = json!({"type":"object","properties":{"task":{"type":"string"},"context":{"type":"string"},"tools":{"type":"array","items":{"type":"string"}}},"required":["task"],"additionalProperties":false});
        if !self.0.templates.is_empty() {
            schema["properties"]["agent"] = json!({"type":"string","enum":self.0.templates.iter().map(|a| &a.name).collect::<Vec<_>>(),"description":self.0.templates.iter().map(|a| format!("{}: {}", a.name, a.description)).collect::<Vec<_>>().join("; ")});
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
        vec![]
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            task: String,
            agent: Option<String>,
            context: Option<String>,
            tools: Option<Vec<String>>,
        }
        let mut input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if let Some(name) = &input.agent {
            let template = self
                .0
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
        let result = match self.0.spawn_agent(
            &input.task,
            SpawnOptions {
                context: input.context,
                tools: input.tools,
                timeout_secs: self.0.controller.child_execution_budget().turn_timeout_secs,
            },
        ) {
            Ok(id) => {
                let _cancellation = CancelOnDrop {
                    manager: Arc::clone(&self.0),
                    id: id.clone(),
                };
                self.0.wait_agent(&id).await
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
    }
    pub fn configure_agent_templates(&mut self, templates: Vec<AgentTemplate>) {
        self.agent_templates = templates;
    }
    /// The optional runtime primitive handle, initialized only on enabled turns.
    #[must_use]
    pub fn subagent_manager(&self) -> Option<Arc<SubagentManager>> {
        self.subagent_manager.clone()
    }
    /// Prepare enabled delegation without performing a model call or provisioning a child.
    pub fn prepare_subagents(&mut self) -> Option<mpsc::UnboundedReceiver<AgentEvent>> {
        if !self.subagent_config.enabled
            || self.subagent_config.max_depth == 0
            || self.child_run.is_some()
        {
            return None;
        }
        let host = self.child_host.clone()?;
        if let Some(previous) = self.subagent_manager.take() {
            previous.shutdown();
        }
        let mut controller = self.fork_with_messages(vec![]);
        controller.tools.remove("subagent");
        controller.child_budget = self.child_budget;
        let (events, receiver) = mpsc::unbounded_channel();
        let manager = Arc::new(SubagentManager {
            controller,
            templates: self.agent_templates.clone(),
            host,
            slots: Semaphore::new(self.subagent_config.max_concurrent.clamp(1, 64)),
            next: AtomicU64::new(1),
            accepting: AtomicBool::new(true),
            pending: Mutex::new(std::collections::HashMap::new()),
            events,
        });
        self.subagent_manager = Some(Arc::clone(&manager));
        self.tools.register(SubagentTool(manager));
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
