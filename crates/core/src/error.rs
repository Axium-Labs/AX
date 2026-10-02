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
    #[error("turn exceeded its time budget: {0}")]
    Timeout(String),
    #[error("waiting for the user to answer: {0}")]
    WaitingForUser(Box<crate::UserQuestion>),
    #[error("history persistence failed: {0}")]
    Persistence(String),
    #[error("agent worker failed: {0}")]
    WorkerJoin(String),
}

impl AgentError {
    /// Suspension is not failure: budget, step limits and user questions leave
    /// the goal resumable, so the queue may not be marked `blocked`.
    #[must_use]
    pub const fn resumable(&self) -> bool {
        matches!(
            self,
            Self::Budget(_) | Self::StepLimit(_) | Self::Timeout(_) | Self::WaitingForUser(_)
        )
    }

    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout(_))
    }
}
