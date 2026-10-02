//! The AX application: decide which command this invocation is and run it.
//!
//! Nothing here constructs a provider, a tool or a session: it routes, and the
//! command modules or the frontends do the work.

use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::execution;
use runtime_core::ExecutionBudget;

use crate::{
    acp,
    args::{Cli, Command},
    auth_login,
    bootstrap::{Globals, Locations},
    capabilities, capability_import, commands, config, crew_device, evolution, model_selection,
    repl::ReplState,
    tui::run_tui,
};

/// How many consecutive `request_user_input` questions one non-interactive run
/// answers before it stops asking. A safeguard against a loop, not a limit the
/// agent can reason about.
const MAX_USER_QUESTIONS: usize = 8;

/// Whether stdin is a terminal. A piped invocation cannot answer a question
/// later, so it should be told to resume the session instead of blocking.
fn is_interactive() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}

/// Read one answer from stdin. `None` means end of input.
fn read_answer() -> Result<Option<String>> {
    let mut line = String::new();
    let read = std::io::stdin()
        .read_line(&mut line)
        .context("failed to read the answer")?;
    Ok((read > 0).then(|| line.trim().to_owned()))
}

/// Commands that must be decided before the sandbox or the storage locations
/// are touched: persisted settings, the environment/shell selection, and the
/// WSL launcher. Returns whether the invocation was fully handled.
///
/// # Errors
/// Returns an error when settings validation or the launcher fails.
pub(crate) fn run_preflight(cli: &Cli) -> Result<bool> {
    if run_settings(cli)? {
        return Ok(true);
    }
    if let Some(Command::Environment {
        environment,
        terminal_shell,
    }) = &cli.command
    {
        execution::configure(*environment, *terminal_shell)?;
        return Ok(true);
    }
    if let Some(code) = execution::launch(cli)? {
        std::process::exit(code);
    }
    Ok(false)
}

/// `ax settings`: validates and optionally persists Subagent settings. Returns
/// whether the command was handled, so the caller keeps one exit path per
/// command.
fn run_settings(cli: &Cli) -> Result<bool> {
    let Some(Command::Settings {
        subagent,
        max_concurrent,
        max_depth,
    }) = &cli.command
    else {
        return Ok(false);
    };
    let (subagent, max_concurrent, max_depth) = (*subagent, *max_concurrent, *max_depth);
    let mut config = config::AxConfig::load()?;
    if let Some(value) = subagent {
        config.subagent.enabled = value;
    }
    if let Some(value) = max_concurrent {
        anyhow::ensure!((1..=64).contains(&value), "max_concurrent must be 1..=64");
        config.subagent.max_concurrent = value;
    }
    if let Some(value) = max_depth {
        anyhow::ensure!(value <= 1, "max_depth must be 0 or 1");
        config.subagent.max_depth = value;
    }
    if subagent.is_some() || max_concurrent.is_some() || max_depth.is_some() {
        config.save()?;
    }
    println!("{}", serde_json::to_string_pretty(&config.subagent)?);
    Ok(true)
}
/// Credential, capability and gateway subcommands. None of them enters the
/// interactive loop, and each returns before any session is opened.
pub(crate) async fn run_control_command(cli: &Cli, locations: &Locations) -> Result<bool> {
    if let Some(Command::Auth { command }) = &cli.command {
        auth_login::run(command).await?;
        return Ok(true);
    }
    if let Some(Command::Capabilities {
        kind,
        action,
        name,
        scope,
        source,
    }) = &cli.command
    {
        let mut state = ReplState::new(
            locations.data_dir.clone(),
            locations.skills_dir.clone(),
            cli.mcp_config.clone(),
        )?;
        let kind = capabilities::Kind::parse(kind)?;
        let scope = capabilities::parse_scope(scope)?;
        if action != "list" {
            state.manage_capability(
                kind,
                scope,
                action,
                name.as_deref().context("name is required")?,
                source.as_deref(),
            )?;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&state.capability_rows(kind, Some(scope))?)?
        );
        return Ok(true);
    }
    if capability_import::run(cli, &locations.data_dir, &locations.skills_dir)? {
        return Ok(true);
    }
    if matches!(cli.command, Some(Command::Acp)) {
        acp::run(
            cli,
            locations.data_dir.clone(),
            locations.skills_dir.clone(),
        )
        .await?;
        return Ok(true);
    }
    if let Some(Command::Crew { command }) = &cli.command {
        crew_device::run(command).await?;
        return Ok(true);
    }
    Ok(false)
}
/// The remaining one-shot subcommands and the interactive loop. The
/// interactive path borrows nothing from the caller's owned paths.
///
/// # Errors
/// Returns whatever the selected command reports.
pub(crate) async fn dispatch(
    cli: &Cli,
    locations: &Locations,
    budget: ExecutionBudget,
    globals: &Globals,
) -> Result<()> {
    let (data_dir, skills_dir, cwd) = (&locations.data_dir, &locations.skills_dir, &locations.cwd);
    match &cli.command {
        Some(
            Command::Acp
            | Command::Environment { .. }
            | Command::Settings { .. }
            | Command::Crew { .. }
            | Command::Auth { .. }
            | Command::Skill { .. }
            | Command::Mcp { .. }
            | Command::Capabilities { .. },
        ) => {
            unreachable!("handled before command dispatch")
        }
        Some(Command::Export {
            path,
            memory,
            sessions,
        }) => {
            commands::backup::run_export(path, data_dir, cwd, *memory, *sessions)?;
        }
        Some(Command::Import { path, dry_run }) => {
            commands::backup::run_import(path, data_dir, cwd, *dry_run)?;
        }
        Some(Command::Run {
            prompt,
            session,
            resume_goal,
            cancel_goal,
        }) => {
            let selection = model_selection::require_resolved(cli)?;
            let mut state =
                ReplState::new(data_dir.clone(), skills_dir.clone(), cli.mcp_config.clone())?;
            state.execution_budget = budget;
            state.child_timeout_secs = cli.child_timeout_secs;
            // Prepare the run before restoring history so the restore is paged
            // against the budget of the tools this run will actually send.
            state.prepare_runtime(&selection)?;
            if let Some(session) = session
                && !state.open_session(session, &state.context_budget(&selection))?
            {
                return Err(anyhow!("AX session not found"));
            }
            state.next_goal_turn = if let Some(goal_id) = resume_goal {
                runtime_core::GoalTurn::Resume {
                    goal_id: goal_id.clone(),
                }
            } else if let Some(goal_id) = cancel_goal {
                runtime_core::GoalTurn::Cancel {
                    goal_id: goal_id.clone(),
                }
            } else {
                runtime_core::GoalTurn::New
            };
            evolution::run_once(
                &mut state,
                &selection,
                Arc::clone(&globals.approval),
                prompt,
            )
            .await?;
            // A run can park on `request_user_input`. Non-interactively there is
            // no UI to answer it later, so this is the one place the answer is
            // read from stdin; the answer resumes the same goal in place.
            for _ in 0..MAX_USER_QUESTIONS {
                let Some(question) = state.pending_question().cloned() else {
                    break;
                };
                if !is_interactive() {
                    eprintln!(
                        "[ax] the run is waiting for your answer; re-run with `--session` and reply to continue:\n{question}"
                    );
                    break;
                }
                eprintln!("[ax] the agent needs your input:\n{question}");
                let Some(answer) = read_answer()? else {
                    break;
                };
                evolution::run_once(
                    &mut state,
                    &selection,
                    Arc::clone(&globals.approval),
                    &answer,
                )
                .await?;
            }
        }
        Some(Command::Agents {
            prompts,
            concurrency,
        }) => {
            let selection = model_selection::require_resolved(cli)?;
            commands::agents::run(
                prompts,
                *concurrency,
                &selection,
                Arc::clone(&globals.approval),
                budget,
                &globals.auth_path,
            )
            .await?;
        }
        Some(Command::Tui) | None => {
            let resolution = model_selection::resolve_model_selection(cli)?;
            // Boxed: the TUI future is the largest one in the process and is
            // created once, so an allocation here keeps every caller's stack
            // frame (including `main`) small.
            Box::pin(run_tui(
                resolution,
                data_dir.clone(),
                skills_dir.clone(),
                cli.mcp_config.clone(),
                cli.allow_dangerous,
                cli.codex_auth.clone(),
                (budget, cli.child_timeout_secs),
            ))
            .await?;
        }
    }
    Ok(())
}
