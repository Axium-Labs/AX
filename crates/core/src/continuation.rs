//! Runtime facts, never prompt classification or semantic completion scoring.
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// A host can steer an active turn without creating a second execution loop.
#[derive(Default)]
struct Inputs {
    messages: Vec<model::Message>,
    closed: bool,
}
#[derive(Clone, Default)]
pub struct TurnInput(Arc<std::sync::Mutex<Inputs>>);
static STEER_ID: AtomicUsize = AtomicUsize::new(0);
impl TurnInput {
    pub fn steer(&self, input: impl Into<String>) {
        let _ = self.try_steer(input);
    }
    /// Returns a stable transcript ID only when the current execution accepts input.
    pub fn try_steer(&self, input: impl Into<String>) -> Option<String> {
        let mut inputs = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inputs.closed {
            return None;
        }
        let id = format!(
            "steer-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            STEER_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut message = model::Message::user(input);
        message.provider_metadata = Some(serde_json::json!({"axSteering":true,"axMessageId":id}));
        inputs.messages.push(message);
        Some(id)
    }
    /// Closing and admission share a lock: a final answer cannot strand accepted input.
    pub(crate) fn close_if_empty(&self) -> bool {
        let mut inputs = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inputs.messages.is_empty() {
            return false;
        }
        inputs.closed = true;
        true
    }
    pub(crate) fn close(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
    }
    pub(crate) fn closed(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed
    }
    pub(crate) fn pending(&self) -> bool {
        !self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .messages
            .is_empty()
    }
    pub(crate) fn drain(&self) -> Vec<model::Message> {
        std::mem::take(
            &mut self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .messages,
        )
    }
}

pub(crate) struct TurnInputLease(pub TurnInput);
impl Drop for TurnInputLease {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[derive(Default)]
pub(crate) struct RuntimeActivity {
    pub approvals: Arc<AtomicUsize>,
    pub retries: Arc<AtomicUsize>,
}

/// A dropped/cancelled future cannot leave a stale pending flag behind.
pub(crate) struct ActivityLease(Arc<AtomicUsize>);
impl ActivityLease {
    pub fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter.clone())
    }
}
impl Drop for ActivityLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct TrackedApproval {
    pub inner: Arc<dyn crate::ApprovalPolicy>,
    pub activity: Arc<RuntimeActivity>,
}
#[async_trait::async_trait]
impl crate::ApprovalPolicy for TrackedApproval {
    async fn approve(
        &self,
        name: &str,
        input: &serde_json::Value,
        permission: tool::ToolPermission,
    ) -> bool {
        let _pending = ActivityLease::new(&self.activity.approvals);
        self.inner.approve(name, input, permission).await
    }
    async fn ask(
        &self,
        name: &str,
        input: &serde_json::Value,
        permission: tool::ToolPermission,
    ) -> bool {
        let _pending = ActivityLease::new(&self.activity.approvals);
        self.inner.ask(name, input, permission).await
    }
    fn capability_decision(
        &self,
        capability: tool::Capability,
    ) -> Option<tool::PermissionDecision> {
        self.inner.capability_decision(capability)
    }
}

// Independent concurrent facts are intentionally flags, projected into one enum below.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TurnState {
    /// Supplied only to optional guards.
    #[serde(skip)]
    pub evidence: Vec<model::Message>,
    #[serde(skip)]
    pub guard_model_requests: Arc<AtomicUsize>,
    #[serde(skip)]
    pub successful_tool_calls: std::collections::BTreeSet<String>,
    pub pending_tool_calls: usize,
    pub pending_tool_results: usize,
    pub pending_tasks: usize,
    pub running_children: usize,
    pub unconsumed_child_results: usize,
    pub pending_approvals: usize,
    pub pending_user_input: bool,
    pub pending_steer: bool,
    pub retry_state: bool,
    pub required_actions: Vec<String>,
    pub model_requests_continuation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationReason {
    ToolCall,
    ToolResult,
    ChildResult,
    Task,
    Steer,
    RequiredAction,
    Model,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    UserInput,
    Approval,
    Child,
    Retry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnContinuation {
    Continue(ContinuationReason),
    Wait(WaitReason),
    Complete,
}

impl TurnState {
    #[must_use]
    pub fn continuation(&self) -> TurnContinuation {
        use ContinuationReason as C;
        use TurnContinuation::{Complete, Continue, Wait};
        use WaitReason as W;
        if self.pending_user_input {
            return Wait(W::UserInput);
        }
        if self.pending_approvals > 0 {
            return Wait(W::Approval);
        }
        if self.retry_state {
            return Wait(W::Retry);
        }
        if self.pending_tool_calls > 0 {
            return Continue(C::ToolCall);
        }
        if self.pending_tool_results > 0 {
            return Continue(C::ToolResult);
        }
        if self.unconsumed_child_results > 0 {
            return Continue(C::ChildResult);
        }
        if self.running_children > 0 {
            return Wait(W::Child);
        }
        if self.pending_steer {
            return Continue(C::Steer);
        }
        if self.pending_tasks > 0 {
            return Continue(C::Task);
        }
        if !self.required_actions.is_empty() {
            return Continue(C::RequiredAction);
        }
        if self.model_requests_continuation {
            return Continue(C::Model);
        }
        Complete
    }
}

#[must_use]
pub fn needs_follow_up(state: &TurnState) -> bool {
    state.continuation() != TurnContinuation::Complete
}

impl crate::AgentKernel {
    #[must_use]
    pub fn with_turn_input(mut self, input: TurnInput) -> Self {
        self.turn_input = input;
        self
    }
    pub fn set_turn_input(&mut self, input: TurnInput) {
        self.turn_input = input;
    }
    #[must_use]
    pub fn turn_input(&self) -> TurnInput {
        self.turn_input.clone()
    }
    /// Snapshot at a model/tool boundary. Scheduler approvals, retries and commands
    /// are awaited inline, so no completion boundary is reachable during them.
    #[must_use]
    pub fn turn_state(&self) -> TurnState {
        let mut state = self.continuation.clone();
        state.pending_approvals = self.activity.approvals.load(Ordering::Acquire);
        state.retry_state = self.activity.retries.load(Ordering::Acquire) > 0;
        state.pending_user_input = self.pending_question.is_some();
        state.pending_steer = self.turn_input.pending();
        state.pending_tasks = self
            .task_queue
            .as_ref()
            .filter(|q| q.active())
            .map_or(0, |q| {
                q.tasks
                    .iter()
                    .filter(|t| {
                        matches!(
                            t.status,
                            crate::task_queue::TaskStatus::Pending
                                | crate::task_queue::TaskStatus::Running
                        )
                    })
                    .count()
            });
        if let Some(manager) = &self.subagent_manager {
            let (running, ready) = manager.continuation_counts();
            state.running_children += running;
            state.unconsumed_child_results += ready;
        }
        state
    }
}
