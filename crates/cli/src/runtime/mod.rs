//! Runtime assembly for the CLI composition root.

mod builder;

pub(crate) use builder::{build_provider, context_budget, execution_budget, kernel, tools};
