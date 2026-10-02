//! The learning data model: experiences, ledger entries and proposed actions.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Step {
    pub tool: String,
    pub detail: String,
    pub success: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Experience {
    pub id: String,
    pub task: String,
    pub intent: String,
    pub tools_used: Vec<String>,
    pub skills_used: Vec<String>,
    pub steps: Vec<Step>,
    pub errors: Vec<String>,
    pub retries: usize,
    /// Exact user text; semantic correction attribution happens only in analysis.
    pub user_corrections: Vec<String>,
    pub success: bool,
    pub project: String,
    pub session: String,
    pub at: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Candidate,
    Trial,
    Active,
    Archived,
    Deleted,
}
impl State {
    pub(crate) fn bucket(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Trial | Self::Active => "live",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub source: String,
    pub created_at: u64,
    pub last_used_at: u64,
    pub use_count: u64,
    pub success_count: u64,
    pub failure_count: u64,
    pub corrections: u64,
    pub confidence: f64,
    pub state: State,
    pub project: String,
    pub digest: String,
    pub epoch: u64,
    pub low_value_reviews: u64,
    pub evidence: Vec<String>,
    #[serde(default)]
    pub trial_baseline: u64,
    #[serde(default)]
    pub trial_success_baseline: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    pub skills: BTreeMap<String, Metadata>,
    pub experiences: Vec<Experience>,
    pub pending: usize,
    pub last_analysis: u64,
    pub epoch: u64,
}

/// The analyzer proposes data only. Paths, code, tools and permissions are absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "UPPERCASE", deny_unknown_fields)]
pub enum Action {
    Create {
        name: String,
        description: String,
        instructions: String,
        evidence: Vec<String>,
        confidence: f64,
    },
    Refine {
        name: String,
        description: String,
        instructions: String,
        evidence: Vec<String>,
        confidence: f64,
        corrections: Vec<String>,
    },
    Merge {
        names: Vec<String>,
        name: String,
        description: String,
        instructions: String,
        evidence: Vec<String>,
        confidence: f64,
    },
    Retire {
        name: String,
    },
    Promote {
        name: String,
        evidence: Vec<String>,
    },
    Memory {
        key: String,
        value: String,
        evidence: Vec<String>,
        quote: String,
        confidence: f64,
    },
    Ignore,
}
