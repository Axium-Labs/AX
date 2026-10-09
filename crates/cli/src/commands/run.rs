//! `ax run` and the shared prompt adapter.
//!
//! `run_prompt` / `run_prompt_with` are the single prompt entry point for the
//! CLI, the TUI and the ACP adapter: they build the kernel lazily, bind the
//! turn's memory/file/skill context and persist the compressed snapshot.

use std::{fs, io::Write, sync::Arc};

use anyhow::{Context, Result};
use model::AuthStorage;
use runtime_core::{AgentEvent, ApprovalPolicy};

use crate::{
    bootstrap::{ax_auth_path, database_path},
    child_runtime, config, evolution, memory_tool,
    model_selection::{ModelSelection, ProviderKind},
    repl::ReplState,
};

fn log_event(event: &AgentEvent) {
    // Opt-in measurement sink; no new session or startup work in normal runs.
    if let Some(path) = std::env::var_os("AX_EVENT_LOG") {
        use std::fs::OpenOptions;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(
                file,
                "{}",
                serde_json::json!({"timestamp":timestamp,"event":event})
            );
        }
    }
}

fn render_event(event: AgentEvent) {
    match event {
        AgentEvent::ModelStarted { provider, model } => {
            eprintln!("[{provider}/{model}] thinking...");
        }
        AgentEvent::ContentDelta { delta } => {
            print!("{delta}");
            let _ = std::io::stdout().flush();
        }
        AgentEvent::ToolStarted { name, detail, .. } => eprintln!("[tool:{name}] {detail}..."),
        AgentEvent::ToolFinished { name, success, .. } => {
            eprintln!("[tool:{name}] {}", if success { "done" } else { "failed" });
        }
        AgentEvent::ContextCompressed {
            removed_messages,
            estimated_tokens_before,
            estimated_tokens_after,
            ..
        } => eprintln!(
            "[memory] compressed {removed_messages} messages ({estimated_tokens_before} -> {estimated_tokens_after} estimated tokens)"
        ),
        AgentEvent::TurnFinished => println!(),
        AgentEvent::UserQuestion { question } => {
            eprintln!("[ax] waiting for your answer:\n{question}");
        }
        AgentEvent::SubagentStarted { id } => eprintln!("[{id}] started"),
        AgentEvent::SubagentCompleted { id } => eprintln!("[{id}] completed"),
        AgentEvent::SubagentFailed { id, error } => eprintln!("[{id}] failed: {error}"),
        AgentEvent::SubagentCancelled { id } => eprintln!("[{id}] cancelled"),
        AgentEvent::Continuation { .. }
        | AgentEvent::Completion { .. }
        | AgentEvent::StopGuardEvaluated { .. }
        | AgentEvent::SubagentProgress { .. }
        | AgentEvent::TurnStarted
        | AgentEvent::ThinkingDelta { .. } => {}
    }
}
pub(crate) async fn run_prompt(
    state: &mut ReplState,
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    prompt: &str,
) -> Result<String> {
    run_prompt_with(state, selection, approval, prompt, render_event).await
}
pub(crate) async fn run_prompt_with<F>(
    state: &mut ReplState,
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    prompt: &str,
    mut emit: F,
) -> Result<String>
where
    F: FnMut(AgentEvent) + Send,
{
    let revision = crate::mods::revision(state)?;
    if state
        .mod_revision
        .as_ref()
        .is_some_and(|previous| *previous != revision)
    {
        state.invalidate_runtime();
    }
    state.mod_revision = Some(revision);
    if state.runtime.is_none() {
        refresh_codex_credential_if_needed(selection).await?;
        // One provider and one final tool registry per run. The prepared parts
        // are the same objects the kernel receives, so the budget measured for
        // this run describes exactly the tools that will be sent.
        state.prepare_runtime(selection)?;
        let prepared = state
            .take_prepared()
            .expect("runtime prepared for this run");
        let runtime = prepared.into_kernel(
            approval.clone(),
            state.loaded_messages.clone(),
            &ax_auth_path(),
        )?;
        state.ensure_session(prompt)?;
        let mut runtime = runtime
            .with_tool(mcp::McpGateway::new(state.mcp()?))
            .with_execution_budget(state.execution_budget);
        if let Some((extension, tools)) =
            crate::mods::ModHost::load(state, runtime.tool_registry(), approval, &selection.model)
                .await?
        {
            for tool in tools {
                runtime.register_tool(tool);
            }
            runtime = runtime.with_runtime_extension(Arc::new(extension));
        }
        state.runtime = Some(runtime);
    } else {
        state.ensure_session(prompt)?;
    }
    child_runtime::configure_controller(state)?;
    if let Some(input) = state.turn_input.take() {
        state
            .runtime
            .as_mut()
            .expect("runtime initialized")
            .set_turn_input(input);
    } else {
        // Each ordinary turn gets a new admission gate; completed handles stay closed.
        state
            .runtime
            .as_mut()
            .expect("runtime initialized")
            .set_turn_input(runtime_core::TurnInput::default());
    }
    if let Some((goal_id, answer)) = answer_intent(state, prompt)? {
        // The submission is the answer to the parked question: resume the same
        // goal from the exact tool call that asked, instead of planning again.
        state.next_goal_turn = runtime_core::GoalTurn::Answer { goal_id, answer };
    } else {
        prepare_turn_context(state, selection, prompt)?;
    }
    let mut runtime = state.runtime.take().expect("runtime initialized");
    let result = evolution::checkpointed_turn(&mut runtime, state, prompt, |event| {
        log_event(&event);
        emit(event);
    })
    .await;
    let snapshot = if runtime.take_compression_dirty() {
        let summary = runtime
            .messages()
            .iter()
            .find(|m| m.content.starts_with("[memory-summary]"))
            .map_or_else(String::new, |m| {
                m.content
                    .trim_start_matches("[memory-summary]\n")
                    .to_owned()
            });
        Some((summary, serde_json::to_string(runtime.messages())))
    } else {
        None
    };
    state.runtime = Some(runtime);
    state.sync_pending_question();
    // Suspending on a question is not a failed turn: report the question as
    // this turn's output so the caller shows it, and let the next submission
    // answer it. The goal stays `waiting_for_user`.
    let result = match result {
        Err(runtime_core::AgentError::WaitingForUser(question)) => Ok(question.to_string()),
        other => other,
    };
    if let Some((summary, effective)) = snapshot {
        // Never advance the watermark past messages that failed to persist.
        if !matches!(result, Err(runtime_core::AgentError::Persistence(_))) {
            let session_id = state.current_session_id()?.to_owned();
            state
                .store()?
                .save_effective_context(&session_id, &summary, &effective?)?;
        }
    }
    result.map_err(Into::into)
}
/// Interpret the submission as the answer to a parked `request_user_input`
/// question. Returns `None` for an ordinary prompt.
///
/// The structured answer is written back to the original tool call by the
/// kernel, so no new user turn is opened and the run resumes in place.
fn answer_intent(
    state: &ReplState,
    prompt: &str,
) -> Result<Option<(String, runtime_core::UserAnswer)>> {
    let Some(question) = state.pending_question() else {
        return Ok(None);
    };
    let goal_id = state
        .runtime
        .as_ref()
        .and_then(runtime_core::AgentKernel::goal_id)
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("the parked question has no goal to resume"))?;
    // An unusable answer is a mistake, not a failure: ask again while the goal
    // stays waiting for the user.
    let answer = runtime_core::UserAnswer::parse(question, prompt).ok_or_else(|| {
        anyhow::anyhow!(
            "answer with one of: {}",
            question
                .options
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    Ok(Some((goal_id, answer)))
}
/// Binds memory access to the current session and request, then fills the
/// retrieved-memory, file and skill context for this turn, all measured against
/// the same `ContextBudget` the request will use.
fn prepare_turn_context(
    state: &mut ReplState,
    selection: &ModelSelection,
    prompt: &str,
) -> Result<()> {
    let global_root = config::ax_home();
    fs::create_dir_all(&global_root)?;
    // Reread each turn so a Personalization change applies on the next turn.
    let personalization = config::AxConfig::load_from_home(&global_root)?.personalization;
    let memory_tool = memory_tool::MemoryTool {
        database: database_path(&state.data_dir),
        global_database: global_root.join("memory.sqlite3"),
        project: state.project_id.clone(),
        session: state.current_session_id()?.to_owned(),
        user_input: prompt.to_owned(),
        creation_disabled: !personalization.tool_memory,
    };
    let runtime = state.runtime.as_mut().expect("runtime initialized");
    if personalization.memory_enabled {
        runtime.register_tool(memory_tool);
    } else {
        runtime.remove_tool("memory");
    }
    let budget = runtime.context_budget();
    runtime.set_context("[retrieved-memory]", None);
    let _timer = tool::telemetry::Timer::new("context.prepare");
    let memory_context = if personalization.memory_enabled {
        state
            .memory_context(prompt, budget.memory_budget_tokens())
            .context("retrieving scoped memory")?
    } else {
        None
    };
    let runtime = state.runtime.as_mut().expect("runtime initialized");
    runtime.set_context("[retrieved-memory]", memory_context);
    runtime.set_context(
        crate::personalization::WRITING_CONTEXT_PREFIX,
        crate::personalization::writing_context(
            &personalization,
            budget.writing_style_budget_tokens(),
        ),
    );
    // Project instructions come from the repository, never from memory or the
    // skill catalog, and occupy their own context slot.
    let segments =
        crate::project_instructions::install(state, prompt, budget.instructions_budget_tokens());
    if segments > 0 {
        tool::telemetry::increment("instructions.installed");
    }
    let remaining = state
        .prepare_file_context(prompt, budget.skills_budget_tokens())
        .context("preparing referenced files")?;
    state.evolution_prepare(selection);
    state
        .prepare_skill_context(prompt, remaining)
        .context("preparing skill context")
}
async fn refresh_codex_credential_if_needed(selection: &ModelSelection) -> Result<()> {
    if !matches!(selection.provider, ProviderKind::Codex) || selection.codex_auth.is_some() {
        return Ok(());
    }
    let storage = AuthStorage::new(ax_auth_path());
    let Some(credential) = storage.resolve_oauth("openai-codex")? else {
        return Ok(());
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if credential.expires > now.saturating_add(60) {
        return Ok(());
    }
    let refreshed = model::refresh_oauth(&credential).await?;
    storage.store_oauth("openai-codex", refreshed)?;
    Ok(())
}
