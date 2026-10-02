//! `ax run` and the shared prompt adapter.
//!
//! `run_prompt` / `run_prompt_with` are the single prompt entry point for the
//! CLI, the TUI and the ACP adapter: they build the kernel lazily, bind the
//! turn's memory/file/skill context and persist the compressed snapshot.

use std::{fs, io::Write, sync::Arc};

use anyhow::Result;
use model::AuthStorage;
use runtime_core::{AgentEvent, ApprovalPolicy};

use crate::{
    bootstrap::{ax_auth_path, database_path},
    child_runtime, config, evolution, memory_tool,
    model_selection::{ModelSelection, ProviderKind},
    repl::ReplState,
    runtime::kernel,
};

fn render_event(event: AgentEvent) {
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
        AgentEvent::SubagentStarted { id } => eprintln!("[{id}] started"),
        AgentEvent::SubagentCompleted { id } => eprintln!("[{id}] completed"),
        AgentEvent::SubagentFailed { id, error } => eprintln!("[{id}] failed: {error}"),
        AgentEvent::SubagentCancelled { id } => eprintln!("[{id}] cancelled"),
        AgentEvent::SubagentProgress { .. }
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
    emit: F,
) -> Result<String>
where
    F: FnMut(AgentEvent) + Send,
{
    if state.runtime.is_none() {
        refresh_codex_credential_if_needed(selection).await?;
        let runtime = kernel(
            selection,
            approval,
            state.loaded_messages.clone(),
            &state.mcp_tools,
            &ax_auth_path(),
        )?;
        state.ensure_session(prompt)?;
        state.runtime = Some(
            runtime
                .with_tool(mcp::McpGateway::new(state.mcp()?))
                .with_execution_budget(state.execution_budget),
        );
    } else {
        state.ensure_session(prompt)?;
    }
    child_runtime::configure_controller(state)?;
    prepare_turn_context(state, selection, prompt)?;
    let mut runtime = state.runtime.take().expect("runtime initialized");
    let result = evolution::checkpointed_turn(&mut runtime, state, prompt, emit).await;
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
    let memory_tool = memory_tool::MemoryTool {
        database: database_path(&state.data_dir),
        global_database: global_root.join("memory.sqlite3"),
        project: state.project_id.clone(),
        session: state.current_session_id()?.to_owned(),
        user_input: prompt.to_owned(),
    };
    let runtime = state.runtime.as_mut().expect("runtime initialized");
    runtime.register_tool(memory_tool);
    let budget = runtime.context_budget();
    runtime.set_context("[retrieved-memory]", None);
    let _timer = tool::telemetry::Timer::new("context.prepare");
    let memory_context = state.memory_context(prompt, budget.memory_budget_tokens())?;
    state
        .runtime
        .as_mut()
        .expect("runtime initialized")
        .set_context("[retrieved-memory]", memory_context);
    let remaining = state.prepare_file_context(prompt, budget.skills_budget_tokens())?;
    state.evolution_prepare(selection);
    state.prepare_skill_context(prompt, remaining)
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
