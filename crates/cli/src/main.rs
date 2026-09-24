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
    AuthStorage, Message, ModelProvider, OpenAiCompatibleConfig, OpenAiCompatibleProvider,
    OpenAiConfig, OpenAiProvider, ReasoningEffort, Role,
};
use runtime_core::{AgentEvent, AgentKernel, AllowAll, ApprovalPolicy, DenyDangerous};
use runtime_core::{AgentSupervisor, AgentTask};
use skill::SkillCatalog;
use tool::{FilesystemTool, ShellTool, ToolRegistry};

mod config;
mod memory_context;
mod memory_tool;
mod model_selection;
mod project_identity;
mod providers;
mod session_restore;
mod skill_settings;
mod tui;
use model_selection::ModelResolution;
use tui::run_tui;

const SKILL_CONTEXT_PREFIX: &str = "[ax-skill:";
const SKILL_CATALOG_PREFIX: &str = "[skill-catalog]";

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
    /// Maximum model steps per turn; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    max_steps: usize,
    /// Maximum tool calls per turn; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    max_tool_calls: usize,
    /// Turn timeout in seconds; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    turn_timeout_secs: u64,
    /// Tool timeout in seconds; 0 (default) means unlimited.
    #[arg(long, global = true, default_value_t = 0)]
    tool_timeout_secs: u64,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Export portable user data to a new .axpack archive.
    Export {
        path: PathBuf,
        #[arg(long)]
        memory: bool,
        #[arg(long)]
        sessions: bool,
    },
    /// Validate and merge a .axpack archive.
    Import {
        path: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
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
    project_local_store: bool,
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
        let project_root = discover_project_root(&std::env::current_dir()?);
        Self::new_in_project(data_dir, skills_dir, mcp_config, &project_root)
    }

    fn new_in_project(
        data_dir: PathBuf,
        skills_dir: PathBuf,
        mcp_config: Option<PathBuf>,
        project_root: &Path,
    ) -> Result<Self> {
        migrate_legacy_project_auth(&data_dir)?;
        fs::create_dir_all(&data_dir)?;
        let project_id = project_identity::load_or_create(project_root)?;
        let database = database_path(&data_dir);
        let mcp_config = mcp_config.unwrap_or_else(|| data_dir.join("mcp.toml"));
        let store = if database.exists() {
            Some(MemoryStore::open(&database).with_context(|| {
                format!("failed to open memory database at {}", database.display())
            })?)
        } else {
            None
        };
        if let Some(store) = &store {
            store.migrate_project_owner(
                &project_id,
                &project_root.to_string_lossy(),
                data_dir == project_root.join(".ax"),
            )?;
        }
        let project_local_store = data_dir == project_root.join(".ax");
        Ok(Self {
            data_dir,
            skills_dir,
            mcp_config,
            store,
            global_store: None,
            memory_scopes_migrated: false,
            project_id,
            project_local_store,
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
        let disabled_skills = self.disabled_skills()?;
        let stored = session_restore::recent_history(self.store()?, id, budget.history_budget())?;
        let summary = self.store()?.session_summary(id)?;
        self.loaded_messages = if let Some(snapshot) = self.store()?.effective_context(id)? {
            serde_json::from_str::<Vec<Message>>(&snapshot)?
        } else {
            summary
                .map(|summary| Message::system(format!("[memory-summary]\n{summary}")))
                .into_iter()
                .collect()
        };
        let has_snapshot = self.store()?.effective_context(id)?.is_some();
        if !has_snapshot {
            let agent_state = self.store()?.load_agent_state_messages(id)?;
            self.loaded_messages
                .extend(agent_state.iter().map(restore_message));
        }
        self.loaded_messages.extend(
            stored
                .iter()
                .filter(|message| has_snapshot || message.kind != MessageKind::AgentState)
                .map(restore_message),
        );
        let recovered = session_restore::interrupted_results(&self.loaded_messages);
        // Persist recovery results without replaying tools with unknown side effects.
        for message in &recovered {
            self.store()?.append_message(
                id,
                NewMessage {
                    role: MessageRole::Tool,
                    kind: MessageKind::ToolCall,
                    content: message.content.clone(),
                    metadata: serde_json::to_value(message)?,
                },
            )?;
        }
        self.loaded_messages.extend(recovered);
        self.loaded_messages.retain(|message| {
            active_skill_name(message).is_none_or(|name| !disabled_skills.contains(&name))
        });
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
        self.persist_turn_messages(messages, &mut 0)
    }

    fn persist_turn_messages(&mut self, messages: &[Message], saved: &mut usize) -> Result<()> {
        let session_id = self.current_session_id()?.to_owned();
        let mcp_call_ids = messages
            .iter()
            .flat_map(|message| &message.tool_calls)
            .filter(|call| call.function.name.starts_with("mcp__"))
            .map(|call| call.id.as_str())
            .collect::<HashSet<_>>();
        for message in messages.iter().skip(*saved) {
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
            *saved += 1;
        }
        Ok(())
    }

    fn skills(&mut self) -> Result<&SkillCatalog> {
        if self.skill_catalog.is_none() {
            let global_skills = config::ax_home().join("skills");
            let catalog = SkillCatalog::index_sources([&self.skills_dir, &global_skills])
                .with_context(|| {
                    format!(
                        "failed to index skills directory {}",
                        self.skills_dir.display()
                    )
                })?;
            for issue in catalog.issues() {
                eprintln!("Skill discovery: {issue}");
            }
            self.skill_catalog = Some(catalog);
        }
        self.skill_catalog
            .as_ref()
            .ok_or_else(|| anyhow!("skill catalog was not initialized"))
    }

    fn skill_tool_names(&self) -> Vec<String> {
        let mut available_tools = tools(&self.mcp_tools)
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if self
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.has_tool("view_image"))
        {
            available_tools.retain(|name| name != "view_image");
        }
        available_tools.push("mcp".to_owned());
        available_tools.push("memory".to_owned());
        available_tools
    }

    /// Expose only descriptions and paths for skills the agent may choose
    /// semantically when automatic metadata ranking does not find a match.
    fn skill_catalog_context(&mut self, token_budget: usize) -> Result<(Option<Message>, usize)> {
        let available_tools = self.skill_tool_names();
        let disabled = self.disabled_skills()?;
        let active = self.active_skills.clone();
        let catalog = self.skills()?;
        let mut content = format!(
            "{SKILL_CATALOG_PREFIX}\nChoose a relevant skill by description; read its file with the filesystem tool. Tool permissions still apply.\n"
        );
        let mut included = false;
        for status in catalog.statuses(available_tools.iter().map(String::as_str)) {
            if !status.available()
                || disabled.contains(&status.metadata.name)
                || active.contains(&status.metadata.name)
            {
                continue;
            }
            let Some(instruction_path) = catalog.instruction_path(&status.metadata.name) else {
                continue;
            };
            let line = format!(
                "{} | {} | {}\n",
                status.metadata.name,
                status.metadata.description,
                instruction_path.display()
            );
            let candidate = Message::system(format!("{content}{line}"));
            if runtime_core::estimate_tokens(std::slice::from_ref(&candidate)) <= token_budget {
                content.push_str(&line);
                included = true;
            }
        }
        if !included {
            return Ok((None, 0));
        }
        let message = Message::system(content);
        let size = runtime_core::estimate_tokens(std::slice::from_ref(&message));
        Ok((Some(message), size))
    }

    fn route_skills(&mut self, prompt: &str, token_budget: usize) -> Result<Vec<Message>> {
        let available_tools = self.skill_tool_names();
        let disabled_skills = self.disabled_skills()?;
        let candidates = self
            .skills()?
            .auto_route_candidates(prompt, available_tools.iter().map(String::as_str));
        let mut messages = Vec::new();
        let mut remaining_tokens = token_budget;
        for matched in candidates {
            if disabled_skills.contains(&matched.name) || self.active_skills.contains(&matched.name)
            {
                continue;
            }
            let loaded = match self.skills()?.load(&matched.name) {
                Ok(loaded) => loaded,
                Err(error) => {
                    eprintln!("Skill loading: {error}");
                    continue;
                }
            };
            let message = Message::system(format!(
                "{SKILL_CONTEXT_PREFIX}{}]\nSkill root: {}\n{}",
                loaded.metadata.name,
                loaded.directory.display(),
                loaded.instructions
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

    fn prepare_skill_context(&mut self, prompt: &str, token_budget: usize) -> Result<()> {
        let skill_messages = self.route_skills(prompt, token_budget)?;
        let catalog_context = if skill_messages.is_empty() {
            self.skill_catalog_context(token_budget)?.0
        } else {
            None
        };
        self.runtime
            .as_mut()
            .ok_or_else(|| anyhow!("agent runtime was not initialized"))?
            .set_context(SKILL_CATALOG_PREFIX, catalog_context);
        for skill_message in skill_messages {
            self.persist_messages(std::slice::from_ref(&skill_message))?;
            self.runtime
                .as_mut()
                .ok_or_else(|| anyhow!("agent runtime was not initialized"))?
                .push_context(skill_message);
        }
        Ok(())
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
    if let Some(root) = candidates
        .iter()
        .find(|dir| dir.join(".ax/project.json").is_file() || dir.join(".ax/project-id").is_file())
    {
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
    registry.register(tool::WebTool::new());
    registry.register(tool::ViewImageTool::new(tool_workspace()));
    for tool in mcp_tools {
        registry.register(tool.clone());
    }
    registry
}

fn tool_workspace() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    discover_project_root(&cwd)
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
            let mut config = model::deepseek_compatible_config(Some(selection.model.clone()), key);
            if let Some(context_window) = selection.context_window {
                config.context_window = context_window;
            }
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiCompatibleProvider::new(config))
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
            let mut config = OpenAiCompatibleConfig::new(
                spec.id,
                selection.model.clone(),
                key,
                endpoint,
                selection.context_capacity(),
            );
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiCompatibleProvider::new(config))
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
            estimated_tokens_after,
            ..
        } => eprintln!(
            "[memory] compressed {removed_messages} messages ({estimated_tokens_before} -> {estimated_tokens_after} estimated tokens)"
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
        max_steps: cli.max_steps,
        max_tool_calls: cli.max_tool_calls,
        turn_timeout_secs: cli.turn_timeout_secs,
        tool_timeout_secs: cli.tool_timeout_secs,
    };
    drop(startup_timer);
    let auth_path = ax_auth_path();
    let approval: Arc<dyn ApprovalPolicy> = if cli.allow_dangerous {
        Arc::new(AllowAll)
    } else {
        Arc::new(DenyDangerous)
    };

    match cli.command {
        Some(Command::Export {
            ref path,
            memory,
            sessions,
        }) => {
            run_export(path, &data_dir, &cwd, memory, sessions)?;
        }
        Some(Command::Import { ref path, dry_run }) => {
            run_import(path, &data_dir, &cwd, dry_run)?;
        }
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

fn run_export(
    path: &Path,
    data_dir: &Path,
    cwd: &Path,
    memory: bool,
    sessions: bool,
) -> Result<()> {
    let project_root = discover_project_root(cwd);
    let project_path = database_path(data_dir);
    let project_id = if project_path.exists() {
        project_identity::load_or_create(&project_root)?
    } else {
        project_identity::load_existing(&project_root)?
            .unwrap_or_else(|| uuid::Uuid::nil().to_string())
    };
    let global_path = config::ax_home().join("memory.sqlite3");
    let project = if project_path.exists() {
        MemoryStore::open(&project_path)?
    } else {
        MemoryStore::open_in_memory()?
    };
    if project_path.exists() {
        project.migrate_project_owner(
            &project_id,
            &project_root.to_string_lossy(),
            data_dir == project_root.join(".ax"),
        )?;
    }
    let global = if global_path.exists() {
        MemoryStore::open(&global_path)?
    } else {
        MemoryStore::open_in_memory()?
    };
    let selection = if memory || sessions {
        memory::backup::ExportSelection { memory, sessions }
    } else {
        memory::backup::ExportSelection::all()
    };
    let report = memory::backup::ExportService {
        project: &project,
        global: &global,
        project_id: &project_id,
    }
    .export(path, selection)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_import(path: &Path, data_dir: &Path, cwd: &Path, dry_run: bool) -> Result<()> {
    let project_root = discover_project_root(cwd);
    let project_path = database_path(data_dir);
    let global_path = config::ax_home().join("memory.sqlite3");
    let report = if dry_run {
        let project_id = project_identity::load_existing(&project_root)?;
        memory::backup::ImportService::dry_run(
            path,
            &project_path,
            &global_path,
            project_id.as_deref(),
        )?
    } else {
        // Reject invalid packages before creating an identity or opening stores.
        let existing_project_id = project_identity::load_existing(&project_root)?;
        memory::backup::ImportService::dry_run(
            path,
            &project_path,
            &global_path,
            existing_project_id.as_deref(),
        )?;
        let project_id = project_identity::load_or_create(&project_root)?;
        if let Some(parent) = project_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut project = MemoryStore::open(&project_path)?;
        memory::backup::ImportService {
            project: &mut project,
            global_path: &global_path,
            target_project_id: &project_id,
        }
        .import(path)?
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(test)]
mod backup_cli_tests {
    use super::*;
    #[test]
    fn parses_export_selection_and_import_dry_run() {
        let export = Cli::try_parse_from(["ax", "export", "backup.axpack", "--memory"]).unwrap();
        assert!(matches!(
            export.command,
            Some(Command::Export {
                memory: true,
                sessions: false,
                ..
            })
        ));
        let import = Cli::try_parse_from(["ax", "import", "backup.axpack", "--dry-run"]).unwrap();
        assert!(matches!(
            import.command,
            Some(Command::Import { dry_run: true, .. })
        ));
    }
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
    // Bind memory access to the current session and user request on every turn.
    let global_root = config::ax_home();
    fs::create_dir_all(&global_root)?;
    let memory_tool = memory_tool::MemoryTool {
        database: database_path(&state.data_dir),
        global_database: global_root.join("memory.sqlite3"),
        project: state.project_id.clone(),
        session: state.current_session_id()?.to_owned(),
        user_input: prompt.to_owned(),
    };
    state
        .runtime
        .as_mut()
        .expect("runtime initialized")
        .register_tool(memory_tool);
    let budget = state
        .runtime
        .as_ref()
        .expect("runtime initialized")
        .context_budget();
    state
        .runtime
        .as_mut()
        .expect("runtime initialized")
        .set_context("[retrieved-memory]", None);
    let context_timer = tool::telemetry::Timer::new("context.prepare");
    let memory_context = state.memory_context(prompt, budget.memory_budget_tokens())?;
    state
        .runtime
        .as_mut()
        .expect("runtime initialized")
        .set_context("[retrieved-memory]", memory_context);
    state.prepare_skill_context(prompt, budget.skills_budget_tokens())?;
    drop(context_timer);
    let mut runtime = state.runtime.take().expect("runtime initialized");
    let mut saved = 0;
    let result = runtime
        .run_turn_checkpointed(prompt, &mut emit, |messages| {
            state
                .persist_turn_messages(messages, &mut saved)
                .map_err(|error| runtime_core::AgentError::Persistence(error.to_string()))
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
        parts: Vec::new(),
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
