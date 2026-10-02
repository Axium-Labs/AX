//! Layered context compression.
//!
//! The pipeline never deletes raw history: it only changes what is fed to the
//! model, and every tier derives its allowance from [`crate::ContextBudget`].

mod pipeline;
pub(crate) mod summary;

pub use pipeline::CompressionResult;
