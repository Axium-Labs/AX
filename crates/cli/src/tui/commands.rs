//! Unified slash-command registry and the four AX presentation families.
#![allow(clippy::doc_markdown)]

mod catalogs;
mod memories;
use catalogs::{mcp_items, open_mcp, open_skills, open_tools, skill_items};

use anyhow::Result;
use model::{
    AuthStorage, ModelInfo, OAuthCredential, PROVIDERS, ProviderAuthKind, ReasoningEffort, stats,
};
use tokio::sync::mpsc;

/// Progress updates from the background, self-contained Codex device-code
/// login, delivered back to the TUI so it never blocks while the user
/// authorizes in a browser.
pub enum LoginUpdate {
    /// Show the verification link + one-time code to the user.
    Prompt(String),
    /// Tokens were persisted to AX's provider-scoped `~/.ax/auth.json`.
    Success(String),
    /// Login failed (network, auth server, or file write).
    Failed(String),
    /// Plain progress line from a background task (for example the outcome of
    /// the model discovery that runs right after a credential is stored).
    Notice(String),
}

use super::bottom_pane::{
    ModalAction,
    model_picker::{ModelPicker, ReasoningPicker},
    secret_input::SecretInput,
    session_picker::SessionPicker,
    session_rename::SessionRename,
    surface::{SurfaceItem, SurfaceView},
};
use super::catalog_refresh;
use super::{App, BottomPane, TranscriptKind};
use crate::config::{AxConfig, InferenceMode};
use crate::{
    model_selection::{ModelSelection, ProviderKind},
    repl::ReplState,
};
use tool::PermissionDecision;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlashPresentation {
    Picker,
    Manager,
    InfoPanel,
    DirectAction,
}

#[derive(Clone, Copy, Debug)]
pub struct SlashCommandDef {
    pub name: &'static str,
    pub description: &'static str,
    pub presentation: SlashPresentation,
    /// Optional usage hint shown next to the command, mirroring pi's
    /// `argumentHint` (e.g. `/model <provider/model>`).
    pub argument_hint: Option<&'static str>,
}
const fn cmd(
    name: &'static str,
    description: &'static str,
    presentation: SlashPresentation,
    argument_hint: Option<&'static str>,
) -> SlashCommandDef {
    SlashCommandDef {
        name,
        description,
        presentation,
        argument_hint,
    }
}

pub static SLASH_COMMANDS: &[SlashCommandDef] = &[
    cmd(
        "/login",
        "connect a model provider",
        SlashPresentation::Picker,
        Some("<provider>"),
    ),
    cmd(
        "/logout",
        "remove provider credentials",
        SlashPresentation::Picker,
        None,
    ),
    cmd(
        "/model",
        "switch model and reasoning mode",
        SlashPresentation::Picker,
        Some("<provider/model>"),
    ),
    cmd(
        "/resume",
        "resume a previous session",
        SlashPresentation::Picker,
        None,
    ),
    cmd(
        "/new",
        "start a new session",
        SlashPresentation::DirectAction,
        None,
    ),
    cmd(
        "/memory",
        "view and manage memory",
        SlashPresentation::Manager,
        None,
    ),
    cmd(
        "/compact",
        "compact current context",
        SlashPresentation::DirectAction,
        None,
    ),
    cmd(
        "/skills",
        "view and manage skills",
        SlashPresentation::Manager,
        None,
    ),
    cmd(
        "/agents",
        "manage scoped agents",
        SlashPresentation::Manager,
        None,
    ),
    cmd(
        "/tools",
        "view available tools",
        SlashPresentation::InfoPanel,
        None,
    ),
    cmd(
        "/mcp",
        "manage MCP servers",
        SlashPresentation::Manager,
        None,
    ),
    cmd(
        "/permissions",
        "configure tool permissions",
        SlashPresentation::Manager,
        None,
    ),
    cmd(
        "/status",
        "show runtime status",
        SlashPresentation::InfoPanel,
        None,
    ),
    cmd(
        "/environment",
        "select agent environment and terminal shell",
        SlashPresentation::Manager,
        None,
    ),
    cmd(
        "/settings",
        "configure optional subagents",
        SlashPresentation::Manager,
        None,
    ),
    cmd("/exit", "exit AX", SlashPresentation::DirectAction, None),
];

pub fn filter_commands(token: &str) -> Vec<&'static SlashCommandDef> {
    let token = token.trim().to_ascii_lowercase();
    SLASH_COMMANDS
        .iter()
        .filter(|c| token.is_empty() || c.name.trim_start_matches('/').contains(&token))
        .collect()
}

fn capability_settings(state: &ReplState) -> Box<dyn super::bottom_pane::PaneView> {
    SurfaceView::manager(
        "Settings",
        "capability-settings",
        vec![format!("Project identity: {}", state.project_id)],
        vec![
            item(
                "global",
                "Global configuration",
                "Available to all projects",
            ),
            item(
                "project",
                "Current project configuration",
                "Includes inherited globals; changes apply here",
            ),
            item(
                "subagents",
                "Subagent execution",
                "Global execution defaults",
            ),
        ],
        "Enter select | Esc close",
    )
}
fn capability_scope_settings(state: &ReplState) -> Box<dyn super::bottom_pane::PaneView> {
    SurfaceView::manager(
        "Capabilities",
        "capability-kinds",
        vec![format!(
            "Scope: {}",
            state
                .capability_scope
                .unwrap_or(scoped::Scope::Project)
                .label()
        )],
        vec![
            item("skills", "Skills", ""),
            item("mcp", "MCP", ""),
            item("agents", "Agents", ""),
        ],
        "Enter manage | Esc back",
    )
}
fn capability_slash(
    command: &str,
    state: &mut ReplState,
) -> Result<Option<crate::capabilities::Kind>> {
    let Some((prefix, rest)) = command.split_once(' ') else {
        return Ok(None);
    };
    let kind = match prefix {
        "/skills" => crate::capabilities::Kind::Skills,
        "/mcp" => crate::capabilities::Kind::Mcp,
        "/agents" => crate::capabilities::Kind::Agents,
        _ => return Ok(None),
    };
    let mut args = rest.split_whitespace();
    let action = args.next().unwrap_or("list");
    if action == "list" {
        state.capability_scope = args
            .next()
            .map(crate::capabilities::parse_scope)
            .transpose()?;
    } else {
        let name = args
            .next()
            .ok_or_else(|| anyhow::anyhow!("name is required"))?;
        let scope = args
            .next()
            .map(crate::capabilities::parse_scope)
            .transpose()?
            .unwrap_or(state.capability_scope.unwrap_or(scoped::Scope::Project));
        let source = args.collect::<Vec<_>>().join(" ");
        state.manage_capability(
            kind,
            scope,
            action,
            name,
            if source.is_empty() {
                None
            } else {
                Some(std::path::Path::new(source.trim_matches('\"')))
            },
        )?;
    }
    Ok(Some(kind))
}

fn subagent_settings() -> Result<Box<dyn super::bottom_pane::PaneView>> {
    let config = AxConfig::load()?;
    Ok(SurfaceView::manager(
        "Settings",
        "subagent_settings",
        vec!["Subagent changes apply on the next agent turn.".into()],
        vec![
            SurfaceItem {
                id: "enabled".into(),
                label: "Subagents (toggle)".into(),
                value: config.subagent.enabled.to_string(),
            },
            SurfaceItem {
                id: "max_concurrent".into(),
                label: "Maximum concurrent subagents (cycle 1–3)".into(),
                value: config.subagent.max_concurrent.to_string(),
            },
            SurfaceItem {
                id: "max_depth".into(),
                label: "Maximum depth (children cannot delegate)".into(),
                value: config.subagent.max_depth.min(1).to_string(),
            },
        ],
        "Enter change · Esc close",
    ))
}
fn save_subagent_setting(enabled: bool, app: &mut App) -> Result<()> {
    let mut config = AxConfig::load()?;
    config.subagent.enabled = enabled;
    config.save()?;
    app.push(
        TranscriptKind::Status,
        "Saved; applies on the next agent turn.",
    );
    Ok(())
}

fn execution_settings() -> Result<Box<dyn super::bottom_pane::PaneView>> {
    let config = AxConfig::load()?;
    Ok(SurfaceView::manager(
        "Execution settings",
        "execution",
        vec!["Changes apply to new AX processes and Crew terminals.".into()],
        vec![
            SurfaceItem {
                id: "environment".into(),
                label: "Agent environment".into(),
                value: format!("{:?}", config.execution.environment),
            },
            SurfaceItem {
                id: "terminal_shell".into(),
                label: "Integrated terminal shell".into(),
                value: format!("{:?}", config.execution.terminal_shell),
            },
        ],
        "Enter select · Esc close",
    ))
}

pub(super) async fn execute_slash(
    command: &str,
    state: &mut ReplState,
    selection: &mut ModelSelection,
    app: &mut App,
    pane: &mut BottomPane,
) -> Result<bool> {
    if let Some(kind) = capability_slash(command.trim(), state)? {
        match kind {
            crate::capabilities::Kind::Skills => open_skills(state, pane)?,
            crate::capabilities::Kind::Mcp => open_mcp(state, pane).await?,
            crate::capabilities::Kind::Agents => catalogs::open_agents(state, pane)?,
        }
        return Ok(true);
    }
    match command.trim() {
        "/exit" => return Ok(false),
        "/login" => open_provider_login(pane),
        "/logout" => open_provider_logout(state, app, pane)?,
        "/model" => open_model_picker(state, selection, app, pane, None),
        "/resume" => open_session_picker(state, app, pane)?,
        "/new" => {
            state.reset_new_session();
            app.reset_for_session("New Session", selection);
            app.push(TranscriptKind::Status, "Started new session");
        }
        "/memory" => pane.push_view(memory_root()),
        "/compact" => {
            let before = state.runtime.as_ref().map_or(
                estimate_loaded(state),
                runtime_core::AgentKernel::estimated_context_tokens,
            );
            app.push(TranscriptKind::Status, format!("Compact current context\nCurrent context  {before} / {}\n• Summarizing conversation…", selection.context_capacity()));
            let result = if let Some(runtime) = state.runtime.as_mut() {
                runtime.set_context("[retrieved-memory]", None);
                runtime.compact_now(|_| {}).await?
            } else {
                None
            };
            if let Some(compression) = result {
                let after = state
                    .runtime
                    .as_ref()
                    .map_or(0, runtime_core::AgentKernel::estimated_context_tokens);
                if let Some(session) = state.current_session.as_ref() {
                    let session_id = session.id.clone();
                    let effective = serde_json::to_string(
                        state.runtime.as_ref().expect("runtime exists").messages(),
                    )?;
                    state.store()?.save_effective_context(
                        &session_id,
                        &compression.summary,
                        &effective,
                    )?;
                    state
                        .runtime
                        .as_mut()
                        .expect("runtime exists")
                        .take_compression_dirty();
                }
                app.push(TranscriptKind::Status, format!("✓ Context compacted\nBefore       {before}\nAfter        {after}\nReduced      {}", before.saturating_sub(after)));
            } else {
                app.push(
                    TranscriptKind::Info,
                    "Nothing to compact yet; AX keeps recent messages verbatim.",
                );
            }
        }
        "/skills" => open_skills(state, pane)?,
        "/tools" => open_tools(state, pane),
        "/mcp" => open_mcp(state, pane).await?,
        "/permissions" => pane.push_view(permissions(&state.permissions)),
        "/status" => pane.push_view(status_panel(state, selection, app)),
        "/settings" => pane.push_view(capability_settings(state)),
        "/agents" => catalogs::open_agents(state, pane)?,
        "/settings subagent on" => save_subagent_setting(true, app)?,
        "/settings subagent off" => save_subagent_setting(false, app)?,
        "/environment" => pane.push_view(execution_settings()?),
        other => {
            if let Some(term) = other.strip_prefix("/model ") {
                open_model_picker(state, selection, app, pane, Some(term.trim()));
            } else {
                app.push(TranscriptKind::Error, format!("Unknown command: {other}"));
            }
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_lines)]
fn open_model_picker(
    state: &mut ReplState,
    selection: &mut ModelSelection,
    app: &mut App,
    pane: &mut BottomPane,
    search: Option<&str>,
) {
    // Only models from configured providers are offered (pi: `available =
    // all.filter(configuredProviders.has(provider))`). A provider counts as
    // configured after resolving AX storage or an explicitly requested legacy
    // credential path. Ambient environment variables do not populate it.
    let codex_auth = selection.codex_auth.clone();
    let mut models = catalog_refresh::cached_snapshot(&state.data_dir, codex_auth.as_ref());
    sort_models(&mut models);

    // `/model <term>`: try an exact match from the snapshot before opening the
    // selector (pi's handleModelCommand → findExactModelReferenceMatch).
    if let Some(term) = search {
        let term = term.trim();
        if !term.is_empty()
            && let Some(model) = find_exact_model(&models, term)
        {
            apply_model_info(selection, state, app, model, None);
            return;
        }
    }

    let (view, refresh_target, notice) = ModelPicker::open_refreshable(
        models,
        &selection.provider_id,
        &selection.model,
        search.unwrap_or(""),
    );
    pane.push_view(view);

    // Share one in-flight refresh across concurrent callers (pi's
    // ModelCatalogRefreshCoordinator); replace the snapshot as a whole and
    // surface the refresh outcome in the picker (pi's refresh status).
    let data_dir = state.data_dir.clone();
    tokio::spawn(async move {
        let refresh = catalog_refresh::refresh_catalogs(data_dir, codex_auth).await;
        let message = if refresh.failed.is_empty() {
            "Model catalogs refreshed.".to_owned()
        } else {
            format!(
                "Could not refresh {}; showing cached models.",
                refresh.failed.join(", ")
            )
        };
        *notice
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message);
        if !refresh.models.is_empty() {
            (*refresh_target
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner))
            .clone_from(&refresh.models);
        }
    });
}

fn sort_models(models: &mut Vec<ModelInfo>) {
    models.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));
    models.dedup_by(|a, b| a.provider == b.provider && a.id == b.id);
}

/// Opens the model picker over the locally cached catalogs of the given
/// providers, used when model resolution found several configured providers.
/// This is purely local — the full `/model` flow can refresh later.
pub(super) fn open_auto_model_picker(providers: &[String], pane: &mut BottomPane) {
    let mut models = providers
        .iter()
        .flat_map(|provider| crate::model_selection::local_catalog_models(provider))
        .collect::<Vec<_>>();
    sort_models(&mut models);
    let (view, _, _) = ModelPicker::open_refreshable(models, "", "", "");
    pane.push_view(view);
}

/// Exact model reference match, following pi's `findExactModelReferenceMatch`:
/// canonical `provider/id`, then split `provider`/`id`, then a bare id that is
/// unique across providers.
fn find_exact_model(models: &[ModelInfo], term: &str) -> Option<ModelInfo> {
    let trimmed = term.trim();
    let lower = trimmed.to_ascii_lowercase();
    let canonical = models
        .iter()
        .filter(|m| format!("{}/{}", m.provider, m.id).to_ascii_lowercase() == lower)
        .collect::<Vec<_>>();
    if canonical.len() == 1 {
        return Some(canonical[0].clone());
    }
    if let Some(slash) = trimmed.find('/') {
        let provider = &trimmed[..slash];
        let id = &trimmed[slash + 1..];
        if !provider.is_empty() && !id.is_empty() {
            let matched = models
                .iter()
                .filter(|m| {
                    m.provider.eq_ignore_ascii_case(provider) && m.id.eq_ignore_ascii_case(id)
                })
                .collect::<Vec<_>>();
            if matched.len() == 1 {
                return Some(matched[0].clone());
            }
        }
    }
    let by_id = models
        .iter()
        .filter(|m| m.id.eq_ignore_ascii_case(&lower))
        .collect::<Vec<_>>();
    (by_id.len() == 1).then(|| by_id[0].clone())
}

fn estimate_loaded(state: &ReplState) -> usize {
    runtime_core::estimate_tokens(&state.loaded_messages)
}

fn memory_root() -> Box<dyn super::bottom_pane::PaneView> {
    SurfaceView::manager(
        "Memory",
        "memory",
        Vec::new(),
        vec![
            item("session", "Session Memory", "current context only"),
            item("project", "Project Memory", "shared in this project"),
            item("global", "Global Memory", "shared across AX"),
        ],
        "↑↓ navigate · Enter open · Esc back",
    )
}

fn permission_items(config: &tool::PermissionStore) -> Vec<SurfaceItem> {
    vec![
        item("shell", "Shell", &config.get("shell").to_string()),
        item(
            "filesystem-write",
            "Filesystem Write",
            &config.get("filesystem-write").to_string(),
        ),
        item(
            "filesystem-read",
            "Filesystem Read",
            &config.get("filesystem-read").to_string(),
        ),
        item("network", "Network", &config.get("network").to_string()),
        item("mcp", "MCP", &config.get("mcp").to_string()),
        item(
            "process",
            "Process Launch",
            &config.get("process").to_string(),
        ),
    ]
}

fn permissions(config: &tool::PermissionStore) -> Box<dyn super::bottom_pane::PaneView> {
    SurfaceView::manager(
        "Permissions",
        "permissions",
        vec!["                         Policy".into()],
        permission_items(config),
        "Enter change · Esc back",
    )
}

fn status_panel(
    state: &mut ReplState,
    selection: &ModelSelection,
    app: &App,
) -> Box<dyn super::bottom_pane::PaneView> {
    let messages = state
        .current_session
        .as_ref()
        .map_or(0, |s| s.message_count);
    let mut lines = vec![
        "Runtime".into(),
        format!("Version           {}", env!("CARGO_PKG_VERSION")),
        "Mode              Agent".into(),
        String::new(),
        "Model".into(),
        format!("Provider          {:?}", selection.provider),
        format!("Model             {}", selection.model),
        format!("Context Window    {}", selection.context_capacity()),
        format!("Tool support      {}", selection.supports_tools),
        String::new(),
        "Session".into(),
        format!("Title             {}", app.session),
        format!("Messages          {messages}"),
        String::new(),
        "Context".into(),
        format!("Usage             {}%", app.context_percent),
        "Compact At        75%".into(),
        String::new(),
        "Environment".into(),
        format!("Directory         {}", app.directory),
    ];
    lines.push(String::new());
    lines.push("Inference".into());
    let inference = AxConfig::load().ok().and_then(|config| config.inference);
    match inference {
        Some(config) if config.mode == InferenceMode::Fast => {
            lines.push("Mode              Fast (adaptive hedging)".into());
            let override_ms = config
                .fast
                .as_ref()
                .and_then(|fast| fast.hedge_threshold_ms);
            let threshold = match override_ms {
                Some(ms) => format!("{ms} ms (configured)"),
                None => format!(
                    "auto {} ms",
                    stats::hedge_threshold(&selection.provider_id, &selection.model).as_millis()
                ),
            };
            lines.push(format!("Hedge Threshold   {threshold}"));
            let snapshot = stats::hedge_snapshot();
            lines.push(format!(
                "Trigger Rate      {:.1}% ({}/{})",
                snapshot.trigger_rate * 100.0,
                snapshot.hedged,
                snapshot.fast_requests
            ));
            let ttft = match (snapshot.ttft_p50_ms, snapshot.ttft_p95_ms) {
                (Some(p50), Some(p95)) => format!("{p50} / {p95} ms"),
                _ => "no data yet".to_owned(),
            };
            lines.push(format!("TTFT P50/P95      {ttft}"));
            lines.push(format!("Tokens/s          {:.1}", snapshot.tokens_per_sec));
            lines.push(format!("Canceled          {}", snapshot.canceled));
            // Measured from actual token spend — never a fixed promise.
            lines.push(format!(
                "Extra Cost        {:.2}x",
                snapshot.extra_cost_factor
            ));
        }
        _ => lines.push("Mode              Standard".into()),
    }
    lines.push(format!(
        "Budget            {} steps / {} tool calls / {} turn / {} tool",
        limit_display(state.execution_budget.max_steps),
        limit_display(state.execution_budget.max_tool_calls),
        timeout_display(state.execution_budget.turn_timeout_secs),
        timeout_display(state.execution_budget.tool_timeout_secs)
    ));
    lines.push("Latency (count / avg ms / max ms)".into());
    for (label, metric) in tool::telemetry::snapshot() {
        lines.push(format!(
            "{label}: {} / {} / {}",
            metric.count,
            metric.total_micros / u128::from(metric.count.max(1)) / 1000,
            metric.max_micros / 1000
        ));
    }
    SurfaceView::info("AX Status", lines)
}

fn limit_display(value: usize) -> String {
    if value == 0 {
        "unlimited".into()
    } else {
        value.to_string()
    }
}

fn timeout_display(seconds: u64) -> String {
    if seconds == 0 {
        "unlimited".into()
    } else {
        format!("{seconds}s")
    }
}

fn item(id: &str, label: &str, value: &str) -> SurfaceItem {
    SurfaceItem {
        id: id.into(),
        label: label.into(),
        value: value.into(),
    }
}

pub(super) fn open_provider_login(pane: &mut BottomPane) {
    pane.push_view(SurfaceView::manager(
        "Select authentication method:",
        "login-auth-type",
        Vec::new(),
        vec![
            item("oauth", "Sign in with an account", "OAuth / subscription"),
            item("api_key", "Sign in with an API key", "provider key"),
        ],
        "Enter select · Esc back",
    ));
}

fn open_login_provider_list(pane: &mut BottomPane, auth_type: &str) -> Result<()> {
    let auth = AuthStorage::new(crate::bootstrap::ax_auth_path());
    let stored = auth.provider_ids()?;
    let items = PROVIDERS
        .iter()
        .filter(|provider| match auth_type {
            "api_key" => provider.auth == ProviderAuthKind::ApiKey,
            "oauth" => {
                provider.auth != ProviderAuthKind::ApiKey
                    || model::provider_supports_oauth(provider.id)
            }
            _ => true,
        })
        .map(|provider| {
            let configured = stored.iter().any(|id| id == provider.id);
            let value = if configured {
                "connected"
            } else {
                match provider.auth {
                    ProviderAuthKind::ApiKey => "API key",
                    ProviderAuthKind::CodexOAuth => "Codex OAuth",
                    ProviderAuthKind::ExternalOAuth => "OAuth",
                    ProviderAuthKind::Ambient => "ambient credentials",
                }
            };
            item(provider.id, provider.name, value)
        })
        .collect();
    pane.push_view(SurfaceView::manager(
        if auth_type == "api_key" {
            "Sign in with an API key"
        } else {
            "Sign in with an account"
        },
        format!("login-provider:{auth_type}"),
        vec!["Type to search · provider credentials are loaded lazily".into()],
        items,
        "Enter connect · Esc back",
    ));
    Ok(())
}

fn open_provider_logout(_state: &ReplState, app: &mut App, pane: &mut BottomPane) -> Result<()> {
    let auth = AuthStorage::new(crate::bootstrap::ax_auth_path());
    let items = auth
        .provider_ids()?
        .into_iter()
        .map(|id| {
            let name = model::provider(&id).map_or(id.as_str(), |provider| provider.name);
            item(&id, name, "AX credential")
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        app.push(
            TranscriptKind::Info,
            "No stored credentials to remove. Environment variables are unchanged.",
        );
        return Ok(());
    }
    pane.push_view(SurfaceView::manager(
        "Log out of a provider",
        "logout-provider",
        vec!["Environment variables are never modified by AX.".into()],
        items,
        "Enter logout · Esc back",
    ));
    Ok(())
}

fn apply_model_info(
    selection: &mut ModelSelection,
    state: &mut ReplState,
    app: &mut App,
    model: ModelInfo,
    effort: Option<ReasoningEffort>,
) {
    let provider =
        match model.provider.as_str() {
            "deepseek" => ProviderKind::Deepseek,
            "openai" => ProviderKind::Openai,
            "codex" | "openai-codex" => ProviderKind::Codex,
            "workbuddy" | "workbuddy-cn" => ProviderKind::Workbuddy,
            provider
                if model::provider(provider).is_some_and(|spec| {
                    spec.protocol == model::ProviderProtocol::OpenAiCompatible
                }) =>
            {
                ProviderKind::Compatible
            }
            provider => {
                app.push(TranscriptKind::Info, format!(
                "{provider} is discoverable, but its native protocol adapter is not enabled yet"
            ));
                return;
            }
        };
    selection.provider = provider;
    selection.provider_id.clone_from(&model.provider);
    selection.endpoint.clone_from(&model.endpoint);
    selection.model = model.id;
    selection.context_window = Some(model.context_window);
    selection.reasoning_effort = effort.or(model.default_reasoning_effort);
    selection.supports_tools = model.supports_tools;
    state.invalidate_runtime();
    app.model.clone_from(&selection.model);
    app.push(
        TranscriptKind::Status,
        format!(
            "Model changed to {} · context {} · tools {}",
            selection.model,
            selection.context_capacity(),
            selection.supports_tools
        ),
    );
    // Remember the successful switch so the next launch resumes it.
    if let Err(error) = crate::model_selection::persist_model_selection(selection) {
        app.push(
            TranscriptKind::Error,
            format!("Could not persist model selection: {error:#}"),
        );
    }
}

pub(super) fn open_session_picker(
    state: &mut ReplState,
    app: &mut App,
    pane: &mut BottomPane,
) -> Result<()> {
    state.register_project()?;
    let sessions = sessions_across_projects(state, crate::session_projects::list()?)?;
    if sessions.is_empty() {
        app.push(TranscriptKind::Info, "No previous sessions");
    } else {
        pane.push_view(SessionPicker::open(sessions));
    }
    Ok(())
}

fn sessions_across_projects(
    state: &mut ReplState,
    projects: Vec<crate::session_projects::ProjectLocation>,
) -> Result<Vec<(memory::Session, crate::session_projects::ProjectLocation)>> {
    let mut sessions = Vec::new();
    for project in projects {
        let database = crate::bootstrap::database_path(&project.data_dir);
        if !database.is_file() {
            continue;
        }
        let listed = if project.id == state.project_id {
            state.store()?.list_sessions(50, 0)?
        } else {
            memory::MemoryStore::open(&database)?.list_sessions(50, 0)?
        };
        sessions.extend(listed.into_iter().map(|session| (session, project.clone())));
    }
    sessions.sort_by(|a, b| b.0.updated_at.cmp(&a.0.updated_at));
    sessions.truncate(100);
    Ok(sessions)
}

fn open_session(
    reference: &str,
    state: &mut ReplState,
    app: &mut App,
    selection: &ModelSelection,
) -> Result<()> {
    let (project_id, id) = reference
        .split_once('|')
        .unwrap_or((&state.project_id, reference));
    if project_id != state.project_id {
        let project = crate::session_projects::list()?
            .into_iter()
            .find(|project| project.id == project_id)
            .ok_or_else(|| anyhow::anyhow!("project not found: {project_id}"))?;
        state.switch_project(&project)?;
    }
    app.directory = state.project_root.display().to_string();
    let budget = crate::runtime::context_budget(selection, &state.mcp_tools);
    if state.open_session(id, &budget)? {
        let history = state
            .store()?
            .load_messages(id, None, crate::session_restore::HISTORY_PAGE_SIZE)?
            .iter()
            .map(crate::repl::restore_message)
            .collect::<Vec<_>>();
        super::restore_transcript(app, &history, selection);
        app.push(TranscriptKind::Status, "Session resumed");
        if state.current_session.as_ref().is_some_and(|session| {
            session.message_count > u64::from(crate::session_restore::HISTORY_PAGE_SIZE)
        }) {
            app.push(TranscriptKind::Info, "Recent transcript loaded. Open /memory → Session → View stored messages for full history.");
        }
    } else {
        app.push(TranscriptKind::Error, format!("Session not found: {id}"));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) async fn apply_modal_action(
    action: ModalAction,
    state: &mut ReplState,
    selection: &mut ModelSelection,
    app: &mut App,
    pane: &mut BottomPane,
    login_tx: &mpsc::UnboundedSender<LoginUpdate>,
) -> Result<()> {
    match action {
        ModalAction::MemoryEdited {
            scope,
            key,
            expected,
            value,
        } => {
            memories::edit(state, &scope, &key, &expected, &value, pane, app)?;
        }
        ModalAction::ModelSelected(model) if model.reasoning_efforts.is_empty() => {
            apply_model_info(selection, state, app, model, None);
        }
        ModalAction::ModelSelected(model) => pane.push_view(ReasoningPicker::open(model)),
        ModalAction::ReasoningSelected { model, effort } => {
            pane.pop_view();
            apply_model_info(selection, state, app, model, Some(effort));
        }
        ModalAction::ApiKeyConfigured { provider, key } => {
            AuthStorage::new(crate::bootstrap::ax_auth_path()).store_api_key(&provider, key)?;
            pane.clear_views();
            state.invalidate_runtime();
            app.push(
                TranscriptKind::Status,
                format!("Saved API key for {provider} · discovering available models"),
            );
            let refresh_data_dir = state.data_dir.clone();
            let refresh_codex_auth = selection.codex_auth.clone();
            let tx = login_tx.clone();
            tokio::spawn(async move {
                // Discover this provider first and report the outcome. A full
                // refresh alone used to be silent: when discovery failed, or
                // when it joined a refresh that started before the key existed,
                // no `~/.ax/models/<provider>.json` appeared and nothing said so.
                let report = |message: String| {
                    let _ = tx.send(LoginUpdate::Notice(message));
                };
                match catalog_refresh::refresh_provider(
                    &refresh_data_dir,
                    refresh_codex_auth.clone(),
                    &provider,
                )
                .await
                {
                    Some(catalog) if catalog.models.is_empty() => report(format!(
                        "{provider}: no models discovered — {}",
                        catalog
                            .warning
                            .unwrap_or_else(|| "provider returned an empty catalog".to_owned())
                    )),
                    Some(catalog) => report(format!(
                        "{provider}: discovered {} model(s), cached for the model picker",
                        catalog.models.len()
                    )),
                    None => report(format!(
                        "{provider}: credential stored, but AX has no usable adapter for it yet"
                    )),
                }
                catalog_refresh::refresh_catalogs(refresh_data_dir, refresh_codex_auth).await;
            });
        }
        ModalAction::SessionOpen(id) => {
            open_session(&id, state, app, selection)?;
            pane.set_file_root(state.project_root.clone());
        }
        ModalAction::SessionNew => {
            state.reset_new_session();
            app.reset_for_session("New Session", selection);
        }
        ModalAction::SessionDelete(id) => {
            let (project_id, session_id) = id
                .split_once('|')
                .unwrap_or((&state.project_id, id.as_str()));
            if project_id == state.project_id {
                state.delete_session(session_id)?;
            } else if let Some(project) = crate::session_projects::list()?
                .into_iter()
                .find(|project| project.id == project_id)
            {
                memory::MemoryStore::open(crate::bootstrap::database_path(&project.data_dir))?
                    .delete_session(session_id)?;
            }
            app.push(
                TranscriptKind::Status,
                format!("Deleted session {session_id}"),
            );
            open_session_picker(state, app, pane)?;
        }
        ModalAction::SessionRenameStart { id, title } => {
            pane.push_view(SessionRename::open(id, title));
        }
        ModalAction::SessionRenamed { id, title } => {
            let (project_id, session_id) = id
                .split_once('|')
                .unwrap_or((&state.project_id, id.as_str()));
            if project_id == state.project_id {
                state.store()?.rename_session(session_id, &title)?;
                if let Some(session) = state
                    .current_session
                    .as_mut()
                    .filter(|session| session.id == session_id)
                {
                    session.title.clone_from(&title);
                }
            } else if let Some(project) = crate::session_projects::list()?
                .into_iter()
                .find(|project| project.id == project_id)
            {
                memory::MemoryStore::open(crate::bootstrap::database_path(&project.data_dir))?
                    .rename_session(session_id, &title)?;
            }
            app.push(
                TranscriptKind::Status,
                format!("Renamed session to {title}"),
            );
            open_session_picker(state, app, pane)?;
        }
        ModalAction::SurfaceSelected { surface, id } => {
            if surface == "capability-settings" {
                if id == "subagents" {
                    pane.push_view(subagent_settings()?);
                } else {
                    state.capability_scope = Some(crate::capabilities::parse_scope(&id)?);
                    pane.push_view(capability_scope_settings(state));
                }
                return Ok(());
            }
            if surface == "capability-kinds" {
                match id.as_str() {
                    "skills" => open_skills(state, pane)?,
                    "mcp" => open_mcp(state, pane).await?,
                    "agents" => catalogs::open_agents(state, pane)?,
                    _ => {}
                }
                return Ok(());
            }
            if surface == "agents" {
                let row = state
                    .capability_rows(crate::capabilities::Kind::Agents, state.capability_scope)?
                    .into_iter()
                    .find(|row| row["name"] == id)
                    .ok_or_else(|| anyhow::anyhow!("Unknown agent"))?;
                state.manage_capability(
                    crate::capabilities::Kind::Agents,
                    state.capability_scope.unwrap_or(scoped::Scope::Project),
                    if row["enabled"] == true {
                        "disable"
                    } else {
                        "enable"
                    },
                    &id,
                    None,
                )?;
                pane.refresh_surface(
                    "agents",
                    &catalogs::capability_items(state, crate::capabilities::Kind::Agents)?,
                );
                return Ok(());
            }
            if surface == "subagent_settings" {
                let mut config = AxConfig::load()?;
                match id.as_str() {
                    "enabled" => config.subagent.enabled = !config.subagent.enabled,
                    "max_concurrent" => {
                        config.subagent.max_concurrent = config.subagent.max_concurrent % 3 + 1;
                    }
                    "max_depth" => {
                        config.subagent.max_depth = usize::from(config.subagent.max_depth == 0);
                    }
                    _ => {}
                }
                config.save()?;
                pane.push_view(subagent_settings()?);
            } else if surface == "execution" {
                let options: Vec<(&str, &str)> = if id == "environment" {
                    vec![
                        ("native", "Windows native"),
                        ("wsl", "Windows Subsystem for Linux"),
                    ]
                } else {
                    vec![
                        ("powershell", "PowerShell"),
                        ("cmd", "Command Prompt"),
                        ("git_bash", "Git Bash"),
                        ("wsl", "WSL"),
                    ]
                };
                pane.push_view(SurfaceView::manager(
                    "Select environment / shell",
                    format!("execution:{id}"),
                    vec![],
                    options
                        .into_iter()
                        .map(|(id, label)| SurfaceItem {
                            id: id.into(),
                            label: label.into(),
                            value: String::new(),
                        })
                        .collect(),
                    "Enter select · Esc back",
                ));
            } else if surface.starts_with("execution:") {
                use crate::config::{AgentEnvironment, TerminalShell};
                let result = if surface == "execution:environment" {
                    crate::execution::select(
                        Some(if id == "wsl" {
                            AgentEnvironment::Wsl
                        } else {
                            AgentEnvironment::Native
                        }),
                        None,
                    )
                } else {
                    crate::execution::select(
                        None,
                        Some(match id.as_str() {
                            "cmd" => TerminalShell::Cmd,
                            "git_bash" => TerminalShell::GitBash,
                            "wsl" => TerminalShell::Wsl,
                            _ => TerminalShell::Powershell,
                        }),
                    )
                };
                match result {
                    Ok(_) => app.push(TranscriptKind::Status, "Saved. Agent environment applies on the next AX launch; terminal shell applies to new Crew terminals."),
                    Err(error) => app.push(TranscriptKind::Error, error.to_string()),
                }
                pane.push_view(execution_settings()?);
            } else if let Some(name) = surface.strip_prefix("memory-items:") {
                memories::open_record(state, name, &id, pane)?;
            } else if let Some(reference) = surface.strip_prefix("memory-record:") {
                memories::action(state, reference, &id, pane, app)?;
            } else if surface == "memory" && id != "session" {
                memories::open_list(state, &id, pane)?;
            } else if surface == "session-memory" {
                match id.as_str() {
                    "compact" => {
                        execute_slash("/compact", state, selection, app, pane).await?;
                    }
                    "facts" => {
                        memories::open_list(state, "session", pane)?;
                    }
                    "summary" => {
                        let summary = match state.current_session.clone() {
                            Some(session) => state
                                .store()?
                                .session_summary(&session.id)?
                                .unwrap_or_else(|| "No summary yet".into()),
                            None => "No active session".into(),
                        };
                        pane.push_view(SurfaceView::info(
                            "Context summary",
                            summary.lines().map(str::to_owned).collect(),
                        ));
                    }
                    "messages" => {
                        let history = match state.current_session.clone() {
                            Some(session) => {
                                state.store()?.load_messages(&session.id, None, u32::MAX)?
                            }
                            None => Vec::new(),
                        };
                        pane.push_view(SurfaceView::info(
                            "Stored history",
                            history
                                .iter()
                                .flat_map(|m| {
                                    format!("{:?}: {}", m.role, m.content)
                                        .lines()
                                        .map(str::to_owned)
                                        .collect::<Vec<_>>()
                                })
                                .collect(),
                        ));
                    }
                    "clear" => {
                        if let Some(session) = state.current_session.clone() {
                            for fact in state.memory_records(memory::MemoryScope::Session)? {
                                state.store()?.forget_scoped(
                                    memory::MemoryScope::Session,
                                    &session.id,
                                    &fact.key,
                                )?;
                            }
                            if let Some(runtime) = state.runtime.as_mut() {
                                runtime.set_context("[retrieved-memory]", None);
                            }
                        }
                        app.push(
                            TranscriptKind::Status,
                            "Session facts cleared; conversation history preserved",
                        );
                    }
                    _ => {}
                }
            } else if surface == "login-auth-type" {
                open_login_provider_list(pane, &id)?;
            } else if let Some(capability) = surface.strip_prefix("permission-choice:") {
                let decision = match id.as_str() {
                    "allow" => PermissionDecision::Allow,
                    "deny" => PermissionDecision::Deny,
                    _ => PermissionDecision::Ask,
                };
                state.permissions.set(capability, decision);
                pane.refresh_surface("permissions", &permission_items(&state.permissions));
                app.push(
                    TranscriptKind::Status,
                    format!("Permission updated: {capability} = {decision}"),
                );
            } else if surface == "skill-toggle" {
                let row = state
                    .capability_rows(crate::capabilities::Kind::Skills, state.capability_scope)?
                    .into_iter()
                    .find(|row| row["name"] == id)
                    .ok_or_else(|| anyhow::anyhow!("Unknown skill"))?;
                state.manage_capability(
                    crate::capabilities::Kind::Skills,
                    state.capability_scope.unwrap_or(scoped::Scope::Project),
                    if row["enabled"] == true {
                        "disable"
                    } else {
                        "enable"
                    },
                    &id,
                    None,
                )?;
                pane.refresh_surface("skills", &skill_items(state)?);
            } else if matches!(surface.as_str(), "mcp-toggle" | "agents-toggle") {
                let kind = if surface == "mcp-toggle" {
                    crate::capabilities::Kind::Mcp
                } else {
                    crate::capabilities::Kind::Agents
                };
                let row = state
                    .capability_rows(kind, state.capability_scope)?
                    .into_iter()
                    .find(|row| row["name"] == id)
                    .ok_or_else(|| anyhow::anyhow!("Unknown capability"))?;
                state.manage_capability(
                    kind,
                    state.capability_scope.unwrap_or(scoped::Scope::Project),
                    if row["enabled"] == true {
                        "disable"
                    } else {
                        "enable"
                    },
                    &id,
                    None,
                )?;
                pane.refresh_surface(kind.key(), &catalogs::capability_items(state, kind)?);
            } else if matches!(surface.as_str(), "skills" | "tools" | "mcp") {
                catalogs::open_detail(&surface, &id, state, pane).await?;
            } else if surface == "mcp-action" {
                let (operation, server) = id.split_once(':').unwrap_or(("", id.as_str()));
                let manager = state.mcp()?;
                if matches!(operation, "x" | "r") {
                    manager.lock().await.disconnect(server);
                    state.mcp_tools.retain(|tool| tool.server() != server);
                }
                // Disconnect also changes the tools exposed to the runtime.
                state.invalidate_runtime();
                if matches!(operation, "c" | "r") {
                    let proxies = match mcp::discover_tool_proxies(manager, server).await {
                        Ok(proxies) => proxies,
                        Err(error) => {
                            pane.refresh_surface("mcp", &mcp_items(state).await?);
                            app.push(TranscriptKind::Error, format!("MCP {server}: {error}"));
                            return Ok(());
                        }
                    };
                    state.mcp_tools.retain(|tool| tool.server() != server);
                    state.mcp_tools.extend(proxies);
                    state.invalidate_runtime();
                }
                pane.refresh_surface("mcp", &mcp_items(state).await?);
                app.push(
                    TranscriptKind::Status,
                    format!(
                        "MCP {server}: {}",
                        match operation {
                            "x" => "sleeping",
                            "r" => "restarted",
                            _ => "connected",
                        }
                    ),
                );
            } else if let Some(auth_type) = surface.strip_prefix("login-provider:") {
                let Some(provider) = model::provider(&id) else {
                    app.push(TranscriptKind::Error, format!("Unknown provider: {id}"));
                    return Ok(());
                };
                if auth_type == "api_key" {
                    pane.push_view(SecretInput::open(id));
                } else {
                    match provider.auth {
                        ProviderAuthKind::CodexOAuth => {
                            pane.clear_views();
                            start_codex_login(app, login_tx, crate::bootstrap::ax_auth_path());
                        }
                        ProviderAuthKind::ApiKey if model::provider_supports_oauth(provider.id) => {
                            pane.clear_views();
                            app.push(
                            TranscriptKind::Info,
                            format!(
                                "{} account login is registered. Its provider-owned OAuth flow is not enabled in this AX build; API-key login remains available.",
                                provider.name
                            ),
                        );
                        }
                        ProviderAuthKind::ApiKey => pane.push_view(SecretInput::open(id)),
                        ProviderAuthKind::ExternalOAuth
                            if model::workbuddy::WorkBuddyRegion::from_provider_id(provider.id)
                                .is_some() =>
                        {
                            pane.clear_views();
                            if app
                                .workbuddy_login
                                .as_ref()
                                .is_some_and(|login| !login.is_finished())
                            {
                                app.push(
                                    TranscriptKind::Info,
                                    "WorkBuddy login is already in progress; press Esc to cancel",
                                );
                                return Ok(());
                            }
                            let region =
                                model::workbuddy::WorkBuddyRegion::from_provider_id(provider.id)
                                    .expect("matched WorkBuddy region");
                            app.workbuddy_login_region = Some(region);
                            let tx = login_tx.clone();
                            app.workbuddy_login = Some(tokio::spawn(async move {
                                let result = crate::auth_login::login_workbuddy(
                                    |text| {
                                        let _ = tx.send(LoginUpdate::Notice(text.to_owned()));
                                    },
                                    false,
                                    region,
                                )
                                .await;
                                let update = match result {
                                    Ok(()) => LoginUpdate::Success(region.provider_id().into()),
                                    Err(error) => {
                                        LoginUpdate::Failed(format!("{}: {error}", region.label()))
                                    }
                                };
                                let _ = tx.send(update);
                            }));
                            app.push(
                                TranscriptKind::Status,
                                format!("Starting {} browser login", region.label()),
                            );
                        }
                        ProviderAuthKind::ExternalOAuth => {
                            pane.clear_views();
                            app.push(
                            TranscriptKind::Info,
                            format!(
                                "{} uses its own OAuth flow. Configure its CLI/token, then reopen /model.",
                                provider.name
                            ),
                        );
                        }
                        ProviderAuthKind::Ambient => {
                            pane.clear_views();
                            app.push(
                            TranscriptKind::Info,
                            format!(
                                "{} uses ambient credentials from its platform CLI or environment.",
                                provider.name
                            ),
                        );
                        }
                    }
                }
            } else if surface == "logout-provider" {
                if app
                    .workbuddy_login_region
                    .is_some_and(|region| region.provider_id() == id)
                    && let Some(login) = app.workbuddy_login.take()
                {
                    login.abort();
                }
                if id == "openai-codex" {
                    AuthStorage::new(crate::bootstrap::ax_auth_path()).remove("openai-codex")?;
                    state.invalidate_runtime();
                    app.push(
                        TranscriptKind::Status,
                        "OpenAI Codex credentials removed from AX auth storage",
                    );
                } else {
                    let removed = AuthStorage::new(crate::bootstrap::ax_auth_path()).remove(&id)?;
                    state.invalidate_runtime();
                    app.push(
                        if removed {
                            TranscriptKind::Status
                        } else {
                            TranscriptKind::Info
                        },
                        if removed {
                            format!("Logged out of {id}")
                        } else {
                            format!("No AX credential stored for {id}; environment was unchanged")
                        },
                    );
                }
                pane.clear_views();
            } else {
                open_surface_detail(&surface, &id, state, selection, pane);
            }
        }
        ModalAction::Approval(_) => {}
    }
    Ok(())
}

/// Start AX's self-contained Codex `device-code` login (ported from OpenAI's
/// open-source `codex-rs/login`, MIT). Runs on a background task so the TUI
/// stays responsive while the user authorizes in a browser; progress is
/// reported back through `login_tx` and rendered into the transcript.
fn start_codex_login(
    app: &mut App,
    login_tx: &mpsc::UnboundedSender<LoginUpdate>,
    auth_path: std::path::PathBuf,
) {
    let tx = login_tx.clone();
    tokio::spawn(async move {
        let report = |update: LoginUpdate| {
            let _ = tx.send(update);
        };
        let auth = match model::begin().await {
            Ok(auth) => auth,
            Err(error) => {
                report(LoginUpdate::Failed(error.to_string()));
                return;
            }
        };
        report(LoginUpdate::Prompt(auth.prompt()));
        match auth.poll_and_exchange().await {
            Ok(tokens) => match AuthStorage::new(auth_path).store_oauth(
                "openai-codex",
                OAuthCredential {
                    access: tokens.access_token,
                    refresh: tokens.refresh_token,
                    expires: tokens.expires_at,
                    account_id: tokens.account_id,
                },
            ) {
                Ok(()) => report(LoginUpdate::Success("openai-codex".into())),
                Err(error) => report(LoginUpdate::Failed(error.to_string())),
            },
            Err(error) => report(LoginUpdate::Failed(error.to_string())),
        }
    });
    app.push(
        TranscriptKind::Status,
        "Starting Codex device-code login — the verification link will appear in the transcript",
    );
}

fn open_surface_detail(
    surface: &str,
    id: &str,
    state: &mut ReplState,
    selection: &ModelSelection,
    pane: &mut BottomPane,
) {
    let view = match surface {
        "memory" if id == "session" => SurfaceView::manager(
            "Session Memory",
            "session-memory",
            vec![
                format!(
                    "Context       {} / {}",
                    estimate_loaded(state),
                    selection.context_capacity()
                ),
                "Threshold     75%".into(),
                "Compression   Auto".into(),
                String::new(),
                "Actions".into(),
            ],
            vec![
                item("facts", "View session facts", ""),
                item("summary", "View summary", ""),
                item("messages", "View stored messages", ""),
                item("compact", "Compact now", ""),
                item("clear", "Clear session facts", "history preserved"),
            ],
            "Esc back",
        ),
        "permissions" => SurfaceView::manager(
            "Permission policy",
            format!("permission-choice:{id}"),
            vec![
                format!("Capability        {id}"),
                format!("Current policy    {}", state.permissions.get(id)),
            ],
            vec![
                item("allow", "Allow", "execute without confirmation"),
                item("ask", "Ask", "require confirmation"),
                item("deny", "Deny", "never allow"),
            ],
            "Enter confirm · Esc back",
        ),
        _ => SurfaceView::info("Details", vec![id.to_owned()]),
    };
    pane.push_view(view);
}

#[cfg(test)]
mod session_picker_tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_sessions_from_two_project_databases() {
        let root = std::env::temp_dir().join(format!("ax-project-picker-{}", uuid::Uuid::new_v4()));
        let a = root.join("a");
        let b = root.join("b");
        fs::create_dir_all(a.join(".ax")).unwrap();
        fs::create_dir_all(b.join(".ax")).unwrap();
        let mut state =
            ReplState::new_in_project(a.join(".ax"), a.join("skills"), None, &a).unwrap();
        state.store().unwrap().create_session("Alpha").unwrap();
        memory::MemoryStore::open(crate::bootstrap::database_path(&b.join(".ax")))
            .unwrap()
            .create_session("Beta")
            .unwrap();
        let make_location =
            |id: String, root: &std::path::Path| crate::session_projects::ProjectLocation {
                id,
                root: root.to_path_buf(),
                data_dir: root.join(".ax"),
                skills_dir: root.join("skills"),
                mcp_config: root.join(".ax/mcp.toml"),
            };
        let current_project_id = state.project_id.clone();
        let listed = sessions_across_projects(
            &mut state,
            vec![
                make_location(current_project_id, &a),
                make_location("b".into(), &b),
            ],
        )
        .unwrap();
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .any(|(session, project)| session.title == "Alpha" && project.root == a)
        );
        assert!(
            listed
                .iter()
                .any(|(session, project)| session.title == "Beta" && project.root == b)
        );
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
}
