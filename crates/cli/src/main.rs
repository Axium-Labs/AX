use std::{
    collections::HashSet,
    fs,
    io::Write,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use mcp::{McpConfig, McpManager, McpToolProxy};
use memory::{MemoryStore, MessageKind, MessageRole, NewMessage, Session, StoredMessage};
use model::{
    AuthStorage, DeepSeekConfig, DeepSeekProvider, Message, ModelProvider, OpenAiConfig,
    OpenAiProvider, ReasoningEffort, Role,
};
use runtime_core::{AgentEvent, AgentKernel, AllowAll, ApprovalPolicy, DenyDangerous};
use runtime_core::{AgentSupervisor, AgentTask};
use skill::SkillCatalog;
use tool::{FilesystemTool, ShellTool, ToolRegistry};

mod config;
mod memory_context;
mod model_selection;
mod providers;
mod tui;
use model_selection::ModelResolution;
use tui::run_tui;

const SKILL_CONTEXT_PREFIX: &str = "[ax-skill:";

#[derive(Parser)]
#[command(
    name = "ax",
    version,
    about = "A fast, lightweight AI agent for the terminal"
)]
struct Cli {
    /// Provider, when chosen explicitly. Otherwise AX resolves the selection
    /// from the persisted config, then local credential detection.
    #[arg(long, global = true)]
    provider: Option<ProviderKind>,
    #[arg(long, global = true)]
    model: Option<String>,
    #[arg(long, global = true)]
    codex_auth: Option<PathBuf>,
    /// Override the active model's context-window token capacity.
    #[arg(long, global = true)]
    context_window: Option<NonZeroUsize>,
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    skills_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    mcp_config: Option<PathBuf>,
    #[arg(long, global = true)]
    allow_dangerous: bool,
    #[arg(long, global = true, default_value = "64")]
    max_steps: NonZeroUsize,
    #[arg(long, global = true, default_value = "128")]
    max_tool_calls: NonZeroUsize,
    #[arg(long, global = true, default_value = "600")]
    turn_timeout_secs: NonZeroUsize,
    #[arg(long, global = true, default_value = "120")]
    tool_timeout_secs: NonZeroUsize,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run one persisted task and exit.
    Run { prompt: String },
    /// Run independent tasks with bounded concurrency.
    Agents {
        #[arg(required = true, num_args = 1..)]
        prompts: Vec<String>,
        #[arg(short, long, default_value_t = 4)]
        concurrency: usize,
    },
    /// Start the inline terminal UI while preserving native scrollback.
    Tui,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProviderKind {
    Deepseek,
    Openai,
    Codex,
    Compatible,
}

#[derive(Clone, Debug)]
struct ModelSelection {
    provider: ProviderKind,
    provider_id: String,
    endpoint: Option<String>,
    model: String,
    codex_auth: Option<PathBuf>,
    context_window: Option<usize>,
    max_output_tokens: Option<usize>,
    reasoning_effort: Option<ReasoningEffort>,
    supports_tools: bool,
}

use tool::{PermissionDecision, PermissionStore};

impl ModelSelection {
    const fn context_capacity(&self) -> usize {
        match self.context_window {
            Some(capacity) => capacity,
            None => match self.provider {
                ProviderKind::Deepseek => 64_000,
                ProviderKind::Openai | ProviderKind::Codex | ProviderKind::Compatible => 200_000,
            },
        }
    }
}

struct ReplState {
    data_dir: PathBuf,
    skills_dir: PathBuf,
    mcp_config: PathBuf,
    store: Option<MemoryStore>,
    global_store: Option<MemoryStore>,
    memory_scopes_migrated: bool,
    project_id: String,
    skill_catalog: Option<SkillCatalog>,
    mcp_manager: Option<Arc<tokio::sync::Mutex<McpManager>>>,
    mcp_tools: Vec<McpToolProxy>,
    active_skills: HashSet<String>,
    current_session: Option<Session>,
    loaded_messages: Vec<Message>,
    runtime: Option<AgentKernel>,
    permissions: PermissionStore,
    execution_budget: runtime_core::ExecutionBudget,
}

impl ReplState {
    fn new(data_dir: PathBuf, skills_dir: PathBuf, mcp_config: Option<PathBuf>) -> Result<Self> {
        migrate_legacy_project_auth(&data_dir)?;
        fs::create_dir_all(&data_dir)?;
        let project_id = discover_project_root(&std::env::current_dir()?)
            .to_string_lossy()
            .into_owned();
        let database = database_path(&data_dir);
        let mcp_config = mcp_config.unwrap_or_else(|| data_dir.join("mcp.toml"));
        let store = if database.exists() {
            Some(MemoryStore::open(&database).with_context(|| {
                format!("failed to open memory database at {}", database.display())
            })?)
        } else {
            None
        };
        Ok(Self {
            data_dir,
            skills_dir,
            mcp_config,
            store,
            global_store: None,
            memory_scopes_migrated: false,
            project_id,
            skill_catalog: None,
            mcp_manager: None,
            mcp_tools: Vec::new(),
            active_skills: HashSet::new(),
            current_session: None,
            loaded_messages: Vec::new(),
            runtime: None,
            permissions: PermissionStore::default(),
            execution_budget: runtime_core::ExecutionBudget::default(),
        })
    }

    fn store(&mut self) -> Result<&mut MemoryStore> {
        if self.store.is_none() {
            fs::create_dir_all(&self.data_dir).with_context(|| {
                format!(
                    "failed to create data directory {}",
                    self.data_dir.display()
                )
            })?;
            let database = database_path(&self.data_dir);
            self.store = Some(MemoryStore::open(&database).with_context(|| {
                format!("failed to open memory database at {}", database.display())
            })?);
        }
        self.store
            .as_mut()
            .ok_or_else(|| anyhow!("memory store was not initialized"))
    }

    fn create_session(&mut self, title: &str) -> Result<()> {
        let session = self.store()?.create_session(title)?;
        self.permissions.reset_session();
        self.current_session = Some(session);
        self.loaded_messages.clear();
        self.active_skills.clear();
        self.runtime = None;
        Ok(())
    }

    fn ensure_session(&mut self, prompt: &str) -> Result<()> {
        if self.current_session.is_none() {
            self.create_session(&title_from_prompt(prompt))?;
        }
        Ok(())
    }

    fn open_session(&mut self, id: &str, budget: &runtime_core::ContextBudget) -> Result<bool> {
        let session = self.store()?.session(id)?;
        let Some(session) = session else {
            return Ok(false);
        };
        let stored = self.store()?.load_context_messages(id)?;
        let summary = self.store()?.session_summary(id)?;
        self.loaded_messages = summary
            .map(|summary| Message::system(format!("[memory-summary]\n{summary}")))
            .into_iter()
            .chain(stored.iter().map(restore_message))
            .collect();
        self.loaded_messages = runtime_core::select_context(
            &self.loaded_messages,
            budget.recent_messages_budget(),
            budget.session_summary_budget(),
        );
        self.active_skills = self
            .loaded_messages
            .iter()
            .filter_map(active_skill_name)
            .collect();
        self.permissions.reset_session();
        self.current_session = Some(session);
        self.runtime = None;
        Ok(true)
    }

    fn delete_session(&mut self, id: &str) -> Result<bool> {
        let deleted = self.store()?.delete_session(id)?;
        if deleted
            && self
                .current_session
                .as_ref()
                .is_some_and(|session| session.id == id)
        {
            self.current_session = None;
            self.loaded_messages.clear();
            self.active_skills.clear();
            self.runtime = None;
        }
        Ok(deleted)
    }

    fn current_session_id(&self) -> Result<&str> {
        self.current_session
            .as_ref()
            .map(|session| session.id.as_str())
            .ok_or_else(|| anyhow!("no active session"))
    }

    fn persist_messages(&mut self, messages: &[Message]) -> Result<()> {
        let session_id = self.current_session_id()?.to_owned();
        let mcp_call_ids = messages
            .iter()
            .flat_map(|message| &message.tool_calls)
            .filter(|call| call.function.name.starts_with("mcp__"))
            .map(|call| call.id.as_str())
            .collect::<HashSet<_>>();
        for message in messages {
            let role = memory_role(&message.role);
            let kind = match message.role {
                Role::System => MessageKind::AgentState,
                Role::Tool
                    if message
                        .tool_call_id
                        .as_deref()
                        .is_some_and(|id| mcp_call_ids.contains(id)) =>
                {
                    MessageKind::McpCall
                }
                Role::Tool => MessageKind::ToolCall,
                Role::User | Role::Assistant
                    if message
                        .tool_calls
                        .iter()
                        .any(|call| call.function.name.starts_with("mcp__")) =>
                {
                    MessageKind::McpCall
                }
                Role::User | Role::Assistant if !message.tool_calls.is_empty() => {
                    MessageKind::ToolCall
                }
                Role::User | Role::Assistant => MessageKind::Message,
            };
            self.store()?.append_message(
                &session_id,
                NewMessage {
                    role,
                    kind,
                    content: message.content.clone(),
                    metadata: serde_json::to_value(message)?,
                },
            )?;
        }
        Ok(())
    }

    fn skills(&mut self) -> Result<&SkillCatalog> {
        if self.skill_catalog.is_none() {
            self.skill_catalog =
                Some(SkillCatalog::index(&self.skills_dir).with_context(|| {
                    format!(
                        "failed to index skills directory {}",
                        self.skills_dir.display()
                    )
                })?);
        }
        self.skill_catalog
            .as_ref()
            .ok_or_else(|| anyhow!("skill catalog was not initialized"))
    }

    fn route_skills(&mut self, prompt: &str, token_budget: usize) -> Result<Vec<Message>> {
        let mut available_tools = tools(&self.mcp_tools)
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        available_tools.push("mcp".to_owned());
        let candidates = self
            .skills()?
            .route_candidates(prompt, available_tools.iter().map(String::as_str));
        let mut messages = Vec::new();
        let mut remaining_tokens = token_budget;
        for matched in candidates {
            if self.active_skills.contains(&matched.name) {
                continue;
            }
            let loaded = self.skills()?.load(&matched.name)?;
            let message = Message::system(format!(
                "{SKILL_CONTEXT_PREFIX}{}]\n{}",
                loaded.metadata.name, loaded.instructions
            ));
            let size = runtime_core::estimate_tokens(std::slice::from_ref(&message));
            if size > remaining_tokens {
                continue;
            }
            remaining_tokens -= size;
            self.active_skills.insert(matched.name.clone());
            messages.push(message);
            if messages.len() == 3 {
                break;
            }
        }
        Ok(messages)
    }

    fn mcp(&mut self) -> Result<Arc<tokio::sync::Mutex<McpManager>>> {
        if self.mcp_manager.is_none() {
            let config = McpConfig::load(&self.mcp_config).with_context(|| {
                format!("failed to load MCP config {}", self.mcp_config.display())
            })?;
            self.mcp_manager = Some(Arc::new(tokio::sync::Mutex::new(McpManager::new(config))));
        }
        self.mcp_manager
            .clone()
            .ok_or_else(|| anyhow!("MCP manager was not initialized"))
    }

    fn invalidate_runtime(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            self.loaded_messages = runtime.messages().to_vec();
        }
    }

    fn reset_new_session(&mut self) {
        self.permissions.reset_session();
        self.current_session = None;
        self.loaded_messages.clear();
        self.active_skills.clear();
        self.runtime = None;
    }
}

fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("memory.sqlite3")
}

/// Recognized project markers, checked when no enclosing Git repository is
/// found. Intentionally small: this only needs to distinguish "a project
/// lives here" from an arbitrary directory, not identify the ecosystem.
const PROJECT_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "Gemfile",
    "composer.json",
];

/// Locates the stable project identity used for Project-scope memory,
/// independent of `--data-dir` (which only controls where AX stores state).
/// Prefers the enclosing Git repository, then a recognized project marker
/// file, and falls back to `start` itself so every invocation still resolves
/// to a concrete, stable path.
///
/// The search stops before the user's home directory so a dotfiles repo or a
/// stray marker file directly under `~` never turns the entire home
/// directory into one giant "project".
fn discover_project_root(start: &Path) -> PathBuf {
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let boundary = home_directory();
    let candidates = start
        .ancestors()
        .take_while(|dir| boundary.as_deref() != Some(*dir))
        .collect::<Vec<_>>();
    if let Some(root) = candidates.iter().find(|dir| dir.join(".git").exists()) {
        return (*root).to_path_buf();
    }
    if let Some(root) = candidates.iter().find(|dir| {
        PROJECT_MARKERS
            .iter()
            .any(|marker| dir.join(marker).exists())
    }) {
        return (*root).to_path_buf();
    }
    start
}

fn resolve_directories(
    cwd: &Path,
    data_dir: Option<PathBuf>,
    skills_dir: Option<PathBuf>,
) -> (PathBuf, PathBuf) {
    let project_root = discover_project_root(cwd);
    (
        data_dir.unwrap_or_else(|| project_root.join(".ax")),
        skills_dir.unwrap_or_else(|| project_root.join("skills")),
    )
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .and_then(|home| fs::canonicalize(home).ok())
}

/// Global AX credential store, following pi's `~/.pi/agent/auth.json`
/// separation from project/session state.
fn ax_auth_path() -> PathBuf {
    config::ax_home().join("auth.json")
}

/// Global model catalogs, matching pi's single user-level model store rather
/// than duplicating provider discovery results in every project.
fn ax_models_dir() -> PathBuf {
    config::ax_home().join("models")
}

/// Older AX builds stored provider credentials inside the current project's
/// data directory. Preserve those logins once, but never import another
/// application's credentials (notably `~/.codex/auth.json`).
fn migrate_legacy_project_auth(data_dir: &Path) -> Result<()> {
    let legacy = data_dir.join("auth.json");
    let target = ax_auth_path();
    if target.exists() || !legacy.is_file() || legacy == target {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&legacy, &target).with_context(|| {
        format!(
            "failed to migrate AX credentials from {} to {}",
            legacy.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn tools(mcp_tools: &[McpToolProxy]) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(ShellTool);
    registry.register(FilesystemTool);
    registry.register(tool::PatchTool);
    registry.register(tool::SearchTool);
    for tool in mcp_tools {
        registry.register(tool.clone());
    }
    registry
}

/// Single source of the context budget available for one turn: reserves room
/// for the reply and the tool schemas that will actually be sent, so history
/// restore, skill instructions, and retrieved memory all share one real
/// accounting of what fits instead of each guessing its own fixed limit.
fn context_budget(
    selection: &ModelSelection,
    mcp_tools: &[McpToolProxy],
) -> runtime_core::ContextBudget {
    let tool_schema_tokens = runtime_core::estimate_tool_schema_tokens(&tools(mcp_tools));
    runtime_core::ContextBudget::new(
        selection.context_capacity(),
        selection.max_output_tokens,
        tool_schema_tokens,
    )
}

fn kernel(
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    messages: Vec<Message>,
    mcp_tools: &[McpToolProxy],
    auth_path: &Path,
) -> Result<AgentKernel> {
    let auth = AuthStorage::new(auth_path);
    let provider: Arc<dyn ModelProvider> = match selection.provider {
        ProviderKind::Deepseek => {
            let key = auth
                .resolve_api_key("deepseek", "DEEPSEEK_API_KEY")?
                .ok_or_else(|| {
                    anyhow!("DeepSeek is not configured; open /model and press A to add an API key")
                })?;
            let mut config = DeepSeekConfig::from_api_key(Some(selection.model.clone()), key);
            if let Some(context_window) = selection.context_window {
                config.context_window = context_window;
            }
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(DeepSeekProvider::new(config))
        }
        ProviderKind::Openai => {
            let key = auth
                .resolve_api_key("openai", "OPENAI_API_KEY")?
                .ok_or_else(|| {
                    anyhow!("OpenAI is not configured; open /model and press A to add an API key")
                })?;
            let mut config = OpenAiConfig::from_api_key(Some(selection.model.clone()), key);
            if let Some(context_window) = selection.context_window {
                config.context_window = context_window;
            }
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiProvider::new(config))
        }
        ProviderKind::Codex => {
            let mut config = if let Some(path) = selection.codex_auth.clone() {
                // Explicit compatibility import only; AX-owned logins live in
                // `~/.ax/auth.json` like pi's provider-scoped store.
                OpenAiConfig::from_codex_auth(Some(selection.model.clone()), Some(path))?
            } else {
                let credential = auth
                    .resolve_oauth("openai-codex")?
                    .ok_or_else(|| anyhow!("OpenAI Codex is not configured; run /login"))?;
                OpenAiConfig::from_oauth(
                    Some(selection.model.clone()),
                    credential.access,
                    credential.account_id,
                )
            };
            if let Some(context_window) = selection.context_window {
                config.context_window = context_window;
            }
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiProvider::new(config))
        }
        ProviderKind::Compatible => {
            let spec = model::provider(&selection.provider_id).ok_or_else(|| {
                anyhow!(
                    "unknown OpenAI-compatible provider: {}",
                    selection.provider_id
                )
            })?;
            let environment = spec
                .environment
                .ok_or_else(|| anyhow!("{} does not use a direct API key", spec.name))?;
            let key = auth
                .resolve_api_key(spec.id, environment)?
                .ok_or_else(|| anyhow!("{} is not configured; run /login", spec.name))?;
            let endpoint = selection
                .endpoint
                .clone()
                .ok_or_else(|| anyhow!("{} catalog did not provide an API endpoint", spec.name))?;
            let mut config = DeepSeekConfig::from_compatible(
                spec.id,
                selection.model.clone(),
                key,
                endpoint,
                selection.context_capacity(),
            );
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(DeepSeekProvider::new(config))
        }
    };
    let tool_registry = if selection.supports_tools {
        tools(mcp_tools)
    } else {
        ToolRegistry::new()
    };
    Ok(AgentKernel::new(provider, tool_registry, approval).with_messages(messages))
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
        AgentEvent::ToolStarted { name } => eprintln!("[tool:{name}] running..."),
        AgentEvent::ToolFinished { name, success } => {
            eprintln!("[tool:{name}] {}", if success { "done" } else { "failed" });
        }
        AgentEvent::ContextCompressed {
            removed_messages,
            estimated_tokens_before,
        } => eprintln!(
            "[memory] compressed {removed_messages} messages ({estimated_tokens_before} estimated tokens)"
        ),
        AgentEvent::TurnFinished => println!(),
        AgentEvent::TurnStarted | AgentEvent::ThinkingDelta { .. } => {}
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let startup_timer = tool::telemetry::Timer::new("startup.resolve");
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    let (data_dir, skills_dir) =
        resolve_directories(&cwd, cli.data_dir.clone(), cli.skills_dir.clone());
    let budget = runtime_core::ExecutionBudget {
        max_steps: cli.max_steps.get(),
        max_tool_calls: cli.max_tool_calls.get(),
        turn_timeout_secs: cli.turn_timeout_secs.get() as u64,
        tool_timeout_secs: cli.tool_timeout_secs.get() as u64,
    };
    drop(startup_timer);
    let auth_path = ax_auth_path();
    let approval: Arc<dyn ApprovalPolicy> = if cli.allow_dangerous {
        Arc::new(AllowAll)
    } else {
        Arc::new(DenyDangerous)
    };

    match cli.command {
        Some(Command::Run { ref prompt }) => {
            let selection = model_selection::require_resolved(&cli)?;
            let mut state = ReplState::new(data_dir, skills_dir, cli.mcp_config.clone())?;
            state.execution_budget = budget;
            run_prompt(&mut state, &selection, approval, prompt).await?;
        }
        Some(Command::Agents {
            ref prompts,
            concurrency,
        }) => {
            let selection = model_selection::require_resolved(&cli)?;
            let template = kernel(&selection, approval, Vec::new(), &[], &auth_path)?
                .with_execution_budget(budget);
            let tasks = prompts
                .iter()
                .enumerate()
                .map(|(index, prompt)| AgentTask {
                    id: format!("agent-{:04}", index + 1),
                    prompt: prompt.clone(),
                    context: Vec::new(),
                })
                .collect();
            let results = AgentSupervisor::new(template, concurrency)
                .run_tasks(tasks, None)
                .await?;
            for result in results {
                match result.result {
                    Ok(output) => println!("[{}]\n{output}\n", result.id),
                    Err(error) => eprintln!("[{}] error: {error}", result.id),
                }
            }
        }
        Some(Command::Tui) | None => {
            let resolution = model_selection::resolve_model_selection(&cli)?;
            run_tui(
                resolution,
                data_dir,
                skills_dir,
                cli.mcp_config.clone(),
                cli.allow_dangerous,
                cli.codex_auth.clone(),
                budget,
            )
            .await?;
        }
    }
    Ok(())
}

async fn run_prompt(
    state: &mut ReplState,
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    prompt: &str,
) -> Result<String> {
    run_prompt_with(state, selection, approval, prompt, render_event).await
}

async fn run_prompt_with<F>(
    state: &mut ReplState,
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    prompt: &str,
    mut emit: F,
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
    state
        .runtime
        .as_mut()
        .expect("runtime initialized")
        .set_context("[retrieved-memory]", None);
    let compression = state
        .runtime
        .as_mut()
        .ok_or_else(|| anyhow!("agent runtime was not initialized"))?
        .compress_if_needed(&mut emit)
        .await?;
    if let Some(compression) = compression {
        let session_id = state.current_session_id()?.to_owned();
        state.store()?.save_context_summary(
            &session_id,
            u32::try_from(compression.retained_messages).unwrap_or(u32::MAX),
            &compression.summary,
            compression.removed_messages,
        )?;
    }
    let context_timer = tool::telemetry::Timer::new("context.prepare");
    let budget = context_budget(selection, &state.mcp_tools);
    let memory_context = state.memory_context(prompt, budget.memory_budget_tokens())?;
    state
        .runtime
        .as_mut()
        .expect("runtime initialized")
        .set_context("[retrieved-memory]", memory_context);
    for skill_message in state.route_skills(prompt, budget.skills_budget_tokens())? {
        state.persist_messages(std::slice::from_ref(&skill_message))?;
        state
            .runtime
            .as_mut()
            .ok_or_else(|| anyhow!("agent runtime was not initialized"))?
            .push_context(skill_message);
    }
    drop(context_timer);
    let runtime = state
        .runtime
        .as_mut()
        .ok_or_else(|| anyhow!("agent runtime was not initialized"))?;
    let start = runtime.messages().len();
    let result = runtime.run_turn(prompt, &mut emit).await;
    let new_messages = runtime.messages()[start..].to_vec();
    state.persist_messages(&new_messages)?;
    result.map_err(Into::into)
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

fn restore_message(stored: &StoredMessage) -> Message {
    if !stored.metadata.is_null()
        && let Ok(message) = serde_json::from_value::<Message>(stored.metadata.clone())
    {
        return message;
    }
    Message {
        role: match stored.role {
            MessageRole::User => Role::User,
            MessageRole::Assistant => Role::Assistant,
            MessageRole::Tool => Role::Tool,
            MessageRole::System => Role::System,
        },
        content: stored.content.clone(),
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

fn active_skill_name(message: &Message) -> Option<String> {
    if message.role != Role::System {
        return None;
    }
    message
        .content
        .strip_prefix(SKILL_CONTEXT_PREFIX)
        .and_then(|content| content.split_once(']'))
        .map(|(name, _)| name.to_owned())
}

const fn memory_role(role: &Role) -> MessageRole {
    match role {
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
        Role::System => MessageRole::System,
    }
}

fn title_from_prompt(prompt: &str) -> String {
    let title = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = title.chars().take(40).collect::<String>();
    if title.is_empty() {
        "Untitled session".to_owned()
    } else {
        title
    }
}

#[cfg(test)]
mod project_root_tests {
    use super::{discover_project_root, resolve_directories};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ax-project-root-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn prefers_the_enclosing_git_repository_over_a_marker_file() {
        let root = temp_dir("git");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        let nested = root.join("crates").join("a");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            discover_project_root(&nested),
            fs::canonicalize(&root).unwrap()
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn falls_back_to_a_marker_file_without_git() {
        let root = temp_dir("marker");
        fs::write(root.join("package.json"), "{}").unwrap();
        let nested = root.join("src");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            discover_project_root(&nested),
            fs::canonicalize(&root).unwrap()
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn falls_back_to_the_starting_directory_when_nothing_is_found() {
        let root = temp_dir("bare");

        assert_eq!(
            discover_project_root(&root),
            fs::canonicalize(&root).unwrap()
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn default_directories_are_stable_from_nested_working_directories() {
        let root = temp_dir("stable-defaults");
        fs::create_dir_all(root.join(".git")).unwrap();
        let nested = root.join("crates").join("core");
        fs::create_dir_all(&nested).unwrap();

        let from_root = resolve_directories(&root, None, None);
        let from_nested = resolve_directories(&nested, None, None);
        assert_eq!(from_root, from_nested);
        assert_eq!(from_root.0, fs::canonicalize(&root).unwrap().join(".ax"));
        assert_eq!(from_root.1, fs::canonicalize(&root).unwrap().join("skills"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn explicit_directories_override_project_defaults() {
        let root = temp_dir("explicit-defaults");
        fs::create_dir_all(root.join(".git")).unwrap();
        let data = PathBuf::from("custom-data");
        let skills = PathBuf::from("custom-skills");

        assert_eq!(
            resolve_directories(&root, Some(data.clone()), Some(skills.clone())),
            (data, skills)
        );

        fs::remove_dir_all(&root).unwrap();
    }
}
