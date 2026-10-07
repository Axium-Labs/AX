//! The provider-agnostic agent runtime kernel.
//!
//! This crate is a façade: the modules below own one responsibility each, and
//! everything the composition root needs is re-exported here. Nothing in this
//! crate knows about a terminal, a configuration file, or a concrete provider.
//!
//! Layout:
//!
//! - `kernel` — kernel state, construction and per-turn goal lifecycle.
//! - `loop_runtime` — the model → tool → model turn loop.
//! - `compression` — the layered context compression pipeline.
//! - `event` / `error` / `approval` — the kernel's outward contracts.
//! - `token` — the single token-estimation primitive every budget derives from.
//! - `budget` / `context` — context limits and context selection.
//! - `scheduler` / [`execution`] / [`child`] / [`child_policy`] /
//!   [`subagent`] / `supervisor` / [`task_queue`] — execution machinery.

mod approval;
mod budget;
pub mod child;
mod child_dispatch;
pub mod child_policy;
pub mod child_result;
mod compression;
mod context;
pub mod continuation;
pub mod stop_guard;
pub use continuation::{
    ContinuationReason, TurnContinuation, TurnState, WaitReason, needs_follow_up,
};
pub use stop_guard::{StopDecision, StopGuard, Verification, VerificationConfig};
mod error;
mod event;
pub mod execution;
mod harness;
pub mod instructions;
mod kernel;
mod loop_hygiene;
mod loop_runtime;
mod runtime_core;
mod scheduler;
pub mod subagent;
mod supervisor;
pub mod task_queue;
mod token;
pub mod user_input;

pub use approval::{AllowAll, ApprovalPolicy, DenyDangerous};
pub use budget::{ContextBudget, ContextDemand, ContextPoolPolicy, ExecutionBudget};
pub use child::{ChildCheckpoint, ChildHost, ChildRun, PreparedChild, terminal_result};
pub use child_policy::ChildPolicy;
pub use child_result::{
    Artifact, ChangedFile, ChildMetrics, ChildResult, ChildStatus, DiffStat, Validation,
    from_durable_json,
};
pub use compression::CompressionResult;
pub use context::select_context;
pub use error::AgentError;
pub use event::{AgentEvent, tool_activity};
pub use execution::{ExecutionState, NoProgressDetector};
pub use instructions::{
    InstructionResolution, InstructionResolver, InstructionScope, InstructionSegment,
};
pub use kernel::AgentKernel;
pub use subagent::{AgentTemplate, SpawnOptions, SubagentConfig, SubagentManager, SubagentResult};
pub use supervisor::{AgentSupervisor, AgentTask, AgentTaskResult, MultiAgentEvent};
pub use task_queue::{GoalTurn, QueueState};
pub use token::{estimate_tokens, estimate_tool_schema_tokens};
pub use user_input::{UserAnswer, UserOption, UserQuestion};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod task_queue_tests;

#[cfg(test)]
mod execution_tests;

#[cfg(test)]
#[path = "../../../test/harness/core.rs"]
mod harness_tests;

#[cfg(test)]
#[path = "../../../test/harness/continuation.rs"]
mod continuation_tests;

// Acceptance suite for the runtime (A–G). The suite drives the real kernel
// loop with scripted providers.
#[cfg(test)]
#[path = "../../../test/harness/runtime_neutrality.rs"]
mod runtime_neutrality_tests;
