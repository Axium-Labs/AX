//! Low-frequency personal learning. No runtime/tool dependency or executable output.
#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
//!
//! Layout:
//!
//! - `config` — the one home for scheduling, scoring and growth limits.
//! - `types` — experiences, ledger entries and proposed actions.
//! - `engine` — the single mutation boundary and the audit trail.
//! - `policy` — the lifecycle decision table the engine enforces.
//! - `storage` — atomic writes, digests and the credential screen.
//! - `worker` — the asynchronous, off-startup analysis thread.

mod config;
mod engine;
mod policy;
mod storage;
mod types;

#[cfg(test)]
#[path = "../../../test/evolution/lifecycle.rs"]
mod tests;
mod worker;

pub use config::Config;
pub use engine::Engine;
pub use storage::now;
pub use types::{Action, Experience, Ledger, Metadata, State, Step};
pub use worker::{Handle, RecordSink, start};

pub const CREATOR: &str = include_str!("../../../skills/skill-creator/SKILL.md");
