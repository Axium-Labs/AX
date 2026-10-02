//! Command families implemented by the CLI itself.
//!
//! Commands that already own a module (`auth_login`, `capabilities`,
//! `capability_import`, `crew_device`, `acp`, `update`, `execution`, `tui`)
//! are routed from [`crate::app`] and are deliberately not re-implemented or
//! re-exported here.

pub(crate) mod agents;
pub(crate) mod backup;
pub(crate) mod run;

#[cfg(test)]
mod prompt_execution_tests;
