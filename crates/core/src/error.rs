//! The kernel's failure taxonomy.

use model::ModelError;
use thiserror::Error;
use tool::ToolError;

/// Everything the agent runtime kernel reports to its caller. Model and tool
/// failures keep their own types through `#[from]` so callers can still match
/// on the underlying cause.
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
