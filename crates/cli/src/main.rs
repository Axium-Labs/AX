use std::{
    collections::{BTreeMap, HashSet},
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
    AuthStorage, DEEPSEEK_FALLBACK_MODEL, DeepSeekConfig, DeepSeekProvider, Message, ModelProvider,
    OPENAI_FALLBACK_MODEL, OpenAiConfig, OpenAiProvider, ReasoningEffort, Role,
};
use runtime_core::{AgentEvent, AgentKernel, AllowAll, ApprovalPolicy, DenyDangerous};
use runtime_core::{AgentSupervisor, AgentTask};
use skill::SkillCatalog;
use tool::{FilesystemTool, ShellTool, ToolRegistry};

mod tui;
use tui::run_tui;

const SESSION_CONTEXT_LIMIT: u32 = 200;
const SKILL_CONTEXT_PREFIX: &str = "[ax-skill:";

#[derive(Parser)]
#[command(
    name = "ax",
    version,
    about = "Lightweight native agent runtime kernel"
)]
struct Cli {
    #[arg(long, value_enum, default_value = "deepseek", global = true)]
    provider: ProviderKind,
    #[arg(long, global = true)]
    model: Option<String>,
    #[arg(long, global = true)]
    codex_auth: Option<PathBuf>,
    /// Override the active model's context-window token capacity.
    #[arg(long, global = true)]
    context_window: Option<NonZeroUsize>,
    #[arg(long, default_value = ".ax", global = true)]
    data_dir: PathBuf,
    #[arg(long, default_value = "skills", global = true)]
    skills_dir: PathBuf,
    #[arg(long, global = true)]
    mcp_config: Option<PathBuf>,
    #[arg(long, global = true)]
    allow_dangerous: bool,
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
    reasoning_effort: Option<ReasoningEffort>,
    supports_tools: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

impl std::fmt::Display for PermissionDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Allow => "Allow",
            Self::Ask => "Ask",
            Self::Deny => "Deny",
        })
    }
}

#[derive(Clone, Debug)]
struct PermissionConfig {
    policies: BTreeMap<String, PermissionDecision>,
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            policies: [
                ("shell", PermissionDecision::Ask),
                ("filesystem-write", PermissionDecision::Ask),
                ("filesystem-read", PermissionDecision::Allow),
                ("network", PermissionDecision::Allow),
                ("mcp", PermissionDecision::Ask),
                ("process", PermissionDecision::Ask),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        }
    }
}

impl PermissionConfig {
    fn get(&self, capability: &str) -> PermissionDecision {
        self.policies
            .get(capability)
            .copied()
            .unwrap_or(PermissionDecision::Ask)
    }
    fn set(&mut self, capability: impl Into<String>, decision: PermissionDecision) {
        self.policies.insert(capability.into(), decision);
    }
    fn capability_for(tool: &str, input: &serde_json::Value) -> &'static str {
        if tool == "shell" {
            "shell"
        } else if tool == "filesystem"
            && input
                .get("operation")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|op| matches!(op, "write" | "delete" | "move" | "copy"))
        {
            "filesystem-write"
        } else if tool == "filesystem" {
            "filesystem-read"
        } else if tool.contains("::") {
            "mcp"
        } else {
            "process"
        }
    }
    fn for_tool(&self, tool: &str, input: &serde_json::Value) -> PermissionDecision {
        self.get(Self::capability_for(tool, input))
    }
}

impl ModelSelection {
    fn from_cli(cli: &Cli) -> Self {
        let model = cli.model.clone().unwrap_or_else(|| match cli.provider {
            ProviderKind::Deepseek => DEEPSEEK_FALLBACK_MODEL.to_owned(),
            ProviderKind::Openai | ProviderKind::Codex | ProviderKind::Compatible => {
                OPENAI_FALLBACK_MODEL.to_owned()
            }
        });
        Self {
            provider: cli.provider,
            provider_id: match cli.provider {
                ProviderKind::Deepseek => "deepseek",
                ProviderKind::Openai => "openai",
                ProviderKind::Codex => "openai-codex",
                ProviderKind::Compatible => "compatible",
            }
            .to_owned(),
            endpoint: None,
            model,
            codex_auth: cli.codex_auth.clone(),
            context_window: cli.context_window.map(NonZeroUsize::get),
            reasoning_effort: None,
            supports_tools: true,
        }
    }

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
    skill_catalog: Option<SkillCatalog>,
    mcp_manager: Option<Arc<tokio::sync::Mutex<McpManager>>>,
    mcp_tools: Vec<McpToolProxy>,
    active_skills: HashSet<String>,
    current_session: Option<Session>,
    loaded_messages: Vec<Message>,
    runtime: Option<AgentKernel>,
    permissions: PermissionConfig,
}

impl ReplState {
    fn new(data_dir: PathBuf, skills_dir: PathBuf, mcp_config: Option<PathBuf>) -> Result<Self> {
        migrate_legacy_project_auth(&data_dir)?;
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
            skill_catalog: None,
            mcp_manager: None,
            mcp_tools: Vec::new(),
            active_skills: HashSet::new(),
            current_session: None,
            loaded_messages: Vec::new(),
            runtime: None,
            permissions: PermissionConfig::default(),
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

    fn open_session(&mut self, id: &str) -> Result<bool> {
        let session = self.store()?.session(id)?;
        let Some(session) = session else {
            return Ok(false);
        };
        let mut stored = self
            .store()?
            .load_messages(id, None, SESSION_CONTEXT_LIMIT)?;
        let recent_ids = stored
            .iter()
            .map(|message| message.id)
            .collect::<HashSet<_>>();
        stored.extend(
            self.store()?
                .load_agent_state_messages(id)?
                .into_iter()
                .filter(|message| !recent_ids.contains(&message.id)),
        );
        stored.sort_by_key(|message| message.id);
        let summary = self.store()?.session_summary(id)?;
        self.loaded_messages = summary
            .map(|summary| Message::system(format!("[memory-summary]\n{summary}")))
            .into_iter()
            .chain(stored.iter().map(restore_message))
            .collect();
        self.active_skills = self
            .loaded_messages
            .iter()
            .filter_map(active_skill_name)
            .collect();
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

    fn route_skill(&mut self, prompt: &str) -> Result<Option<Message>> {
        let available_tools = tools(&self.mcp_tools)
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let matched = self
            .skills()?
            .route(prompt, available_tools.iter().map(String::as_str));
        let Some(matched) = matched else {
            return Ok(None);
        };
        if self.active_skills.contains(&matched.name) {
            return Ok(None);
        }
        let loaded = self.skills()?.load(&matched.name)?;
        self.active_skills.insert(matched.name.clone());
        eprintln!("[skill:{}] loaded", matched.name);
        Ok(Some(Message {
            role: Role::System,
            content: format!(
                "{SKILL_CONTEXT_PREFIX}{}]\n{}",
                loaded.metadata.name, loaded.instructions
            ),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }))
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
        self.current_session = None;
        self.loaded_messages.clear();
        self.active_skills.clear();
        self.runtime = None;
    }
}

fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("memory.sqlite3")
}

/// Global AX credential store, following pi's `~/.pi/agent/auth.json`
/// separation from project/session state.
fn ax_auth_path() -> PathBuf {
    if let Some(root) = std::env::var_os("AX_HOME") {
        return PathBuf::from(root).join("auth.json");
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".ax")
        .join("auth.json")
}

/// Global model catalogs, matching pi's single user-level model store rather
/// than duplicating provider discovery results in every project.
fn ax_models_dir() -> PathBuf {
    ax_auth_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("models")
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
    for tool in mcp_tools {
        registry.register(tool.clone());
    }
    registry
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
    let cli = Cli::parse();
    let selection = ModelSelection::from_cli(&cli);
    let auth_path = ax_auth_path();
    let approval: Arc<dyn ApprovalPolicy> = if cli.allow_dangerous {
        Arc::new(AllowAll)
    } else {
        Arc::new(DenyDangerous)
    };

    match cli.command {
        Some(Command::Run { prompt }) => {
            let mut state = ReplState::new(cli.data_dir, cli.skills_dir, cli.mcp_config)?;
            run_prompt(&mut state, &selection, approval, &prompt).await?;
        }
        Some(Command::Agents {
            prompts,
            concurrency,
        }) => {
            let template = kernel(&selection, approval, Vec::new(), &[], &auth_path)?;
            let tasks = prompts
                .into_iter()
                .enumerate()
                .map(|(index, prompt)| AgentTask {
                    id: format!("agent-{:04}", index + 1),
                    prompt,
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
            run_tui(
                selection,
                cli.data_dir,
                cli.skills_dir,
                cli.mcp_config,
                cli.allow_dangerous,
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
        state.runtime = Some(runtime);
    } else {
        state.ensure_session(prompt)?;
    }
    if let Some(skill_message) = state.route_skill(prompt)? {
        state.persist_messages(std::slice::from_ref(&skill_message))?;
        state
            .runtime
            .as_mut()
            .ok_or_else(|| anyhow!("agent runtime was not initialized"))?
            .push_context(skill_message);
    }
    let compression = state
        .runtime
        .as_mut()
        .ok_or_else(|| anyhow!("agent runtime was not initialized"))?
        .compress_if_needed(&mut emit)
        .await?;
    if let Some(compression) = compression {
        let session_id = state.current_session_id()?.to_owned();
        state.store()?.replace_old_messages_with_summary(
            &session_id,
            u32::try_from(compression.retained_messages).unwrap_or(u32::MAX),
            &compression.summary,
            compression.removed_messages,
        )?;
    }
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
