//! Runtime assembly for the CLI composition root.

mod builder;

pub(crate) use builder::{Runtime, build_provider, execution_budget, kernel, tools};
