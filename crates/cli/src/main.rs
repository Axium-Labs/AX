//! AX entry point: parse arguments, bootstrap the process, dispatch, exit.
//!
//! This file is the composition root's front door and nothing else. Every
//! command implementation lives in [`commands`] or its own module, all session
//! state lives in [`repl`], and runtime assembly lives in [`runtime`].

mod acp;
mod acp_workspace;
mod app;
mod args;
mod auth_login;
mod bootstrap;
mod capabilities;
mod capability_import;
#[cfg(test)]
mod child_benchmark_tests;
mod child_runtime;
mod commands;
mod config;
mod computer_use;
mod host_permissions;
mod crew_device;
mod distributed_client;
mod distributed_host;
mod distributed_tool;
mod distributed_worker;
#[cfg(test)]
mod e2e_tests;
mod evolution;
mod execution;
mod file_reference;
mod memory_context;
mod memory_tool;
mod model_selection;
mod mods;
mod personalization;
mod project_identity;
mod project_instructions;
mod providers;
mod repl;
mod runtime;
mod session_projects;
mod session_restore;
mod skill_invocation;
mod skill_settings;
mod storage_location;
mod subagent_settings;
mod system_info;
mod tui;
mod update;
mod worktree_changes;

use anyhow::{Result, anyhow};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let mut raw_args = std::env::args().skip(1);
    let internal = raw_args.next();
    if let Some(result) = bootstrap::run_internal_entry(internal.as_deref(), &mut raw_args).await {
        return result;
    }
    let startup_timer = tool::telemetry::Timer::new("startup.resolve");
    let cli = args::Cli::parse();
    if cli.update {
        if cli.command.is_some() {
            return Err(anyhow!("--update cannot be combined with a subcommand"));
        }
        return update::run().await;
    }
    if app::run_preflight(&cli)? {
        return Ok(());
    }
    let locations = bootstrap::prepare(&cli)?;
    let budget = runtime::execution_budget(&cli);
    drop(startup_timer);
    let globals = bootstrap::globals(&cli);
    if app::run_control_command(&cli, &locations).await? {
        return Ok(());
    }
    let result = app::dispatch(&cli, &locations, budget, &globals).await;
    mods::close_all().await;
    result
}
