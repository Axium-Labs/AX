//! Session state shared by the CLI, TUI and ACP frontends.
//!
//! `ReplState` owns everything one open session needs: the memory store, the
//! lazily built skill catalog, MCP manager, capability registries and the
//! agent runtime. Every heavy resource is built on first use, never at
//! construction.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow};
use mcp::{McpConfig, McpManager, McpToolProxy};
use memory::{MemoryStore, MessageKind, MessageRole, NewMessage, Session, StoredMessage};
use model::{Message, Role};
use runtime_core::AgentKernel;
use skill::SkillCatalog;
use tool::PermissionStore;

use crate::{
    bootstrap::{database_path, discover_project_root, migrate_legacy_project_auth},
    capabilities, config, file_reference, project_identity,
    runtime::tools,
    session_projects, session_restore, skill_invocation, storage_location,
};

pub(crate) const SKILL_CONTEXT_PREFIX: &str = "[ax-skill:";
const SKILL_CATALOG_PREFIX: &str = "[skill-catalog]";
pub(crate) struct ReplState {
    pub(crate) project_root: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) skills_dir: PathBuf,
    pub(crate) mcp_config: PathBuf,
    pub(crate) mcp_override: Option<McpConfig>,
    pub(crate) store: Option<MemoryStore>,
    pub(crate) global_store: Option<MemoryStore>,
    pub(crate) memory_scopes_migrated: bool,
    pub(crate) project_id: String,
    pub(crate) project_local_store: bool,
    pub(crate) skill_catalog: Option<SkillCatalog>,
    pub(crate) capability_registries: std::cell::RefCell<
        std::collections::HashMap<
            capabilities::Kind,
            scoped::ScopedRegistry<capabilities::Capability>,
        >,
    >,
    pub(crate) capability_scope: Option<scoped::Scope>,
    pub(crate) capability_home: PathBuf,
    pub(crate) file_index: Option<Vec<String>>,
    pub(crate) mcp_manager: Option<Arc<tokio::sync::Mutex<McpManager>>>,
    pub(crate) mcp_tools: Vec<McpToolProxy>,
    pub(crate) active_skills: HashSet<String>,
    pub(crate) allowed_skills: Option<HashSet<String>>,
    pub(crate) current_session: Option<Session>,
    pub(crate) loaded_messages: Vec<Message>,
    pub(crate) runtime: Option<AgentKernel>,
    pub(crate) permissions: PermissionStore,
    pub(crate) execution_budget: runtime_core::ExecutionBudget,
    pub(crate) next_goal_turn: runtime_core::GoalTurn,
    pub(crate) child_timeout_secs: u64,
    pub(crate) evolution: Option<::evolution::Handle>,
    pub(crate) evolution_revision: u64,
}
impl ReplState {
    pub(crate) fn new(
        data_dir: PathBuf,
        skills_dir: PathBuf,
        mcp_config: Option<PathBuf>,
    ) -> Result<Self> {
        let project_root = discover_project_root(&std::env::current_dir()?);
        Self::new_in_project(data_dir, skills_dir, mcp_config, &project_root)
    }

    pub(crate) fn new_in_project(
        data_dir: PathBuf,
        skills_dir: PathBuf,
        mcp_config: Option<PathBuf>,
        project_root: &Path,
    ) -> Result<Self> {
        migrate_legacy_project_auth(&data_dir)?;
        fs::create_dir_all(&data_dir)?;
        let project_id = project_identity::load_or_create(project_root)?;
        storage_location::migrate_project(project_root, &data_dir)?;
        let database = database_path(&data_dir);
        let mcp_config = mcp_config.unwrap_or_else(|| {
            let canonical = project_root.join(".ax/mcp.toml");
            let legacy = data_dir.join("mcp.toml");
            if !canonical.exists() && legacy.exists() {
                legacy
            } else {
                canonical
            }
        });
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
                storage_location::is_project_store(&data_dir, project_root),
            )?;
        }
        let project_local_store = storage_location::is_project_store(&data_dir, project_root);
        Ok(Self {
            project_root: project_root.to_path_buf(),
            data_dir,
            skills_dir,
            mcp_config,
            mcp_override: None,
            store,
            global_store: None,
            memory_scopes_migrated: false,
            project_id,
            project_local_store,
            skill_catalog: None,
            capability_registries: std::cell::RefCell::new(std::collections::HashMap::new()),
            capability_scope: None,
            capability_home: config::ax_home(),
            file_index: None,
            mcp_manager: None,
            mcp_tools: Vec::new(),
            active_skills: HashSet::new(),
            allowed_skills: None,
            current_session: None,
            loaded_messages: Vec::new(),
            runtime: None,
            permissions: PermissionStore::default(),
            execution_budget: runtime_core::ExecutionBudget::default(),
            next_goal_turn: runtime_core::GoalTurn::New,
            child_timeout_secs: 0,
            evolution: None,
            evolution_revision: 0,
        })
    }

    pub(crate) fn store(&mut self) -> Result<&mut MemoryStore> {
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

    pub(crate) fn create_session(&mut self, title: &str) -> Result<()> {
        self.evolution_end_session();
        #[cfg(not(test))]
        self.register_project()?;
        let session = self.store()?.create_session(title)?;
        self.permissions.reset_session();
        self.current_session = Some(session);
        self.loaded_messages.clear();
        self.active_skills.clear();
        self.runtime = None;
        Ok(())
    }

    pub(crate) fn project_location(&self) -> session_projects::ProjectLocation {
        let cwd = std::env::current_dir().unwrap_or_else(|_| self.project_root.clone());
        let absolute = |path: &Path| {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            }
        };
        session_projects::ProjectLocation {
            id: self.project_id.clone(),
            root: self.project_root.clone(),
            data_dir: absolute(&self.data_dir),
            skills_dir: absolute(&self.skills_dir),
            mcp_config: absolute(&self.mcp_config),
        }
    }

    pub(crate) fn register_project(&self) -> Result<()> {
        session_projects::register(self.project_location())
    }

    pub(crate) fn switch_project(
        &mut self,
        location: &session_projects::ProjectLocation,
    ) -> Result<()> {
        if self.project_id == location.id {
            return Ok(());
        }
        self.evolution_end_session();
        let mut next = Self::new_in_project(
            storage_location::relocated_data_dir(location),
            location.skills_dir.clone(),
            Some(location.mcp_config.clone()),
            &location.root,
        )?;
        next.execution_budget = self.execution_budget;
        next.child_timeout_secs = self.child_timeout_secs;
        if location.root.is_dir() {
            std::env::set_current_dir(&location.root)?;
        } else {
            eprintln!(
                "Original workspace {} no longer exists; using the current directory.",
                location.root.display()
            );
        }
        *self = next;
        Ok(())
    }

    pub(crate) fn ensure_session(&mut self, prompt: &str) -> Result<()> {
        if self.current_session.is_none() {
            self.create_session(&title_from_prompt(prompt))?;
        }
        Ok(())
    }

    pub(crate) fn open_session(
        &mut self,
        id: &str,
        budget: &runtime_core::ContextBudget,
    ) -> Result<bool> {
        self.evolution_end_session();
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
            self.loaded_messages.extend(
                agent_state
                    .iter()
                    .filter(|m| {
                        !m.content
                            .starts_with(runtime_core::task_queue::STATE_PREFIX)
                    })
                    .map(restore_message),
            );
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
        let evolved_prefix = self.evolution_root().to_string_lossy().into_owned();
        self.loaded_messages.retain(|message| {
            if message.content.starts_with(SKILL_CONTEXT_PREFIX)
                && message.content.contains(&evolved_prefix)
            {
                return false;
            }
            active_skill_name(message).is_none_or(|name| {
                !disabled_skills.contains(&name)
                    && self
                        .allowed_skills
                        .as_ref()
                        .is_none_or(|allowed| allowed.contains(&name))
            })
        });
        self.loaded_messages.retain(|m| {
            !m.content
                .starts_with(runtime_core::task_queue::STATE_PREFIX)
        });
        self.loaded_messages = runtime_core::select_context(
            &self.loaded_messages,
            budget.recent_messages_budget(),
            budget.session_summary_budget(),
        );
        // Queue state is durable orchestration metadata, independent of context
        // snapshots and token selection. Restore the latest checkpoint even when
        // it predates the bounded recent-history page.
        let queue_state = self
            .store()?
            .latest_agent_state(id, runtime_core::task_queue::STATE_PREFIX)?;
        self.loaded_messages.retain(|m| {
            !m.content
                .starts_with(runtime_core::task_queue::STATE_PREFIX)
        });
        if let Some(queue_state) = queue_state {
            self.loaded_messages.push(restore_message(&queue_state));
        }
        let execution_state = self
            .store()?
            .latest_agent_state(id, runtime_core::execution::STATE_PREFIX)?;
        self.loaded_messages.retain(|m| {
            m.role != Role::System || !m.content.starts_with(runtime_core::execution::STATE_PREFIX)
        });
        if let Some(state) = execution_state {
            self.loaded_messages.push(restore_message(&state));
        }
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

    pub(crate) fn delete_session(&mut self, id: &str) -> Result<bool> {
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

    pub(crate) fn current_session_id(&self) -> Result<&str> {
        self.current_session
            .as_ref()
            .map(|session| session.id.as_str())
            .ok_or_else(|| anyhow!("no active session"))
    }

    pub(crate) fn persist_messages(&mut self, messages: &[Message]) -> Result<()> {
        self.persist_turn_messages(messages, &mut 0)
    }

    pub(crate) fn persist_turn_messages(
        &mut self,
        messages: &[Message],
        saved: &mut usize,
    ) -> Result<()> {
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

    pub(crate) fn skills(&mut self) -> Result<&SkillCatalog> {
        if self.skill_catalog.is_none() {
            let registry = self.capability_registry(capabilities::Kind::Skills)?;
            let catalog = SkillCatalog::index_directories(
                registry
                    .effective()
                    .map(|entry| entry.value.source.as_path()),
            )?;
            self.skill_catalog = Some(catalog);
        }
        self.skill_catalog
            .as_ref()
            .ok_or_else(|| anyhow!("skill catalog was not initialized"))
    }

    pub(crate) fn skill_tool_names(&self) -> Vec<String> {
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
    #[cfg(test)]
    pub(crate) fn skill_catalog_context(
        &mut self,
        token_budget: usize,
    ) -> Result<(Option<Message>, usize)> {
        self.ranked_skill_catalog_context(token_budget, &[])
    }
    pub(crate) fn ranked_skill_catalog_context(
        &mut self,
        token_budget: usize,
        ranking: &[String],
    ) -> Result<(Option<Message>, usize)> {
        let available_tools = self.skill_tool_names();
        let disabled = self.disabled_skills()?;
        let active = self.active_skills.clone();
        let allowed = self.allowed_skills.clone();
        let catalog = self.skills()?;
        let mut content = format!(
            "{SKILL_CATALOG_PREFIX}\nChoose whether a skill is relevant from its metadata; explicitly call invoke_skill with its name to load instructions. Tool permissions still apply.\n"
        );
        let mut included = false;
        let mut statuses = catalog.statuses(available_tools.iter().map(String::as_str));
        statuses.sort_by_key(|s| {
            ranking
                .iter()
                .position(|n| n == &s.metadata.name)
                .unwrap_or(usize::MAX)
        });
        for status in statuses {
            if !status.available()
                || disabled.contains(&status.metadata.name)
                || active.contains(&status.metadata.name)
                || allowed
                    .as_ref()
                    .is_some_and(|allowed| !allowed.contains(&status.metadata.name))
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

    #[allow(clippy::unused_self, clippy::unnecessary_wraps)] // Legacy caller API is intentionally a no-op.
    pub(crate) fn route_skills(
        &mut self,
        _prompt: &str,
        _token_budget: usize,
    ) -> Result<Vec<Message>> {
        Ok(Vec::new()) // Main model selects metadata and lazily reads instructions.
    }

    pub(crate) fn prepare_skill_context(
        &mut self,
        prompt: &str,
        token_budget: usize,
    ) -> Result<()> {
        let available = self.skill_tool_names();
        let disabled = self.disabled_skills()?;
        let allowed = self.allowed_skills.clone();
        let catalog = Arc::new(self.skills()?.metadata_snapshot());
        let eligible = catalog
            .route_candidates(prompt, available.iter().map(String::as_str))
            .into_iter()
            .filter(|m| {
                !disabled.contains(&m.name) && allowed.as_ref().is_none_or(|a| a.contains(&m.name))
            })
            .map(|m| m.name)
            .collect();
        self.runtime
            .as_mut()
            .ok_or_else(|| anyhow!("agent runtime was not initialized"))?
            .register_tool(skill_invocation::SkillInvocation {
                catalog,
                allowed: eligible,
            });
        let skill_messages = self.route_skills(prompt, token_budget)?;
        let catalog_context = if skill_messages.is_empty() {
            let eligible: HashSet<_> = self
                .skills()?
                .route_candidates(prompt, available.iter().map(String::as_str))
                .into_iter()
                .map(|m| m.name)
                .collect();
            let original = self.allowed_skills.clone();
            self.allowed_skills = Some(original.as_ref().map_or_else(
                || eligible.clone(),
                |a| a.intersection(&eligible).cloned().collect(),
            ));
            let ranking = self
                .skills()?
                .route_candidates(prompt, available.iter().map(String::as_str))
                .into_iter()
                .map(|m| m.name)
                .collect::<Vec<_>>();
            let result = self.ranked_skill_catalog_context(token_budget, &ranking);
            self.allowed_skills = original;
            result?.0
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

    pub(crate) fn prepare_file_context(
        &mut self,
        prompt: &str,
        mut token_budget: usize,
    ) -> Result<usize> {
        if !prompt.contains('@') || token_budget == 0 {
            return Ok(token_budget);
        }
        if self.file_index.is_none() {
            self.file_index = Some(file_reference::files(&self.project_root));
        }
        let paths = file_reference::references(prompt, self.file_index.as_deref().unwrap_or(&[]));
        for path in paths {
            let Some(content) = file_reference::read(&self.project_root, &path) else {
                continue;
            };
            let message = Message::system(format!("[file-reference: {path}]\n{content}"));
            let cost = runtime_core::estimate_tokens(std::slice::from_ref(&message));
            if cost > token_budget {
                continue;
            }
            token_budget -= cost;
            self.persist_messages(std::slice::from_ref(&message))?;
            self.runtime
                .as_mut()
                .expect("runtime initialized")
                .push_context(message);
        }
        Ok(token_budget)
    }

    pub(crate) fn mcp(&mut self) -> Result<Arc<tokio::sync::Mutex<McpManager>>> {
        if self.mcp_manager.is_none() {
            let config = if let Some(config) = self.mcp_override.clone() {
                config
            } else {
                self.effective_mcp_config()?
            };
            self.mcp_manager = Some(Arc::new(tokio::sync::Mutex::new(McpManager::new(config))));
        }
        self.mcp_manager
            .clone()
            .ok_or_else(|| anyhow!("MCP manager was not initialized"))
    }

    pub(crate) fn invalidate_runtime(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            self.loaded_messages = runtime.messages().to_vec();
            self.loaded_messages.extend(runtime.task_queue_snapshot());
        }
    }

    pub(crate) fn reset_new_session(&mut self) {
        self.evolution_end_session();
        self.permissions.reset_session();
        self.current_session = None;
        self.loaded_messages.clear();
        self.active_skills.clear();
        self.runtime = None;
    }
}
pub(crate) fn restore_message(stored: &StoredMessage) -> Message {
    if !stored.metadata.is_null()
        && let Ok(message) = serde_json::from_value::<Message>(stored.metadata.clone())
    {
        return message;
    }
    Message {
        usage: None,
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
pub(crate) fn active_skill_name(message: &Message) -> Option<String> {
    if message.role != Role::System {
        return None;
    }
    message
        .content
        .strip_prefix(SKILL_CONTEXT_PREFIX)
        .and_then(|content| content.split_once(']'))
        .map(|(name, _)| name.to_owned())
}
pub(crate) const fn memory_role(role: &Role) -> MessageRole {
    match role {
        Role::User => MessageRole::User,
        Role::Assistant => MessageRole::Assistant,
        Role::Tool => MessageRole::Tool,
        Role::System => MessageRole::System,
    }
}
pub(crate) fn title_from_prompt(prompt: &str) -> String {
    let first_line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let first_clause = first_line
        .split(['。', '！', '？', '.', '!', '?', '\n'])
        .next()
        .unwrap_or("");
    let title = first_clause
        .split_whitespace()
        .filter(|word| !word.starts_with('@'))
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.chars().take(28).collect::<String>();
    if title.is_empty() {
        "Untitled session".to_owned()
    } else {
        title
    }
}
#[cfg(test)]
mod session_title_tests {
    use super::title_from_prompt;
    #[test]
    pub(crate) fn first_task_becomes_short_title() {
        assert_eq!(
            title_from_prompt("检查 @README 中的安装说明。然后修复脚本"),
            "检查 中的安装说明"
        );
        assert!(title_from_prompt("a".repeat(100).as_str()).chars().count() <= 28);
    }
}
#[cfg(test)]
mod file_context_tests {
    use super::*;
    use async_trait::async_trait;
    use model::{ModelError, ModelProvider, ModelRequest, ModelResponse};
    use runtime_core::AllowAll;
    use tool::ToolRegistry;

    struct MockProvider;
    #[async_trait]
    impl ModelProvider for MockProvider {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn model_id(&self) -> &'static str {
            "mock"
        }
        fn context_window(&self) -> usize {
            10_000
        }
        async fn complete(
            &self,
            _: ModelRequest,
        ) -> std::result::Result<ModelResponse, ModelError> {
            Ok(ModelResponse {
                usage: None,
                content: String::new(),
                tool_calls: Vec::new(),
                finish_reason: None,
            })
        }
    }

    #[test]
    pub(crate) fn queue_restore_uses_latest_durable_state_beyond_snapshot_and_history_page() {
        let root = std::env::temp_dir().join(format!("ax-queue-resume-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let mut state =
            ReplState::new_in_project(root.join(".ax"), root.join("skills"), None, &root).unwrap();
        state.ensure_session("queue test").unwrap();
        let session = state.current_session_id().unwrap().to_owned();
        let checkpoint = |status: &str| {
            Message::system(format!(
                "{}{}",
                runtime_core::task_queue::STATE_PREFIX,
                serde_json::json!({
                    "overall_goal":"original goal", "summarized":false,"stop_reason":null,
                    "tasks":[{"title":"one","status":status,"failure_reason":null,"outcome":null},
                             {"title":"two","status":if status == "completed" {"running"} else {"pending"},"failure_reason":null,"outcome":null}]
                })
            ))
        };
        state.persist_messages(&[checkpoint("running")]).unwrap();
        state
            .store()
            .unwrap()
            .save_effective_context(&session, "old summary", "[]")
            .unwrap();
        state.persist_messages(&[checkpoint("completed")]).unwrap();
        for _ in 0..300 {
            state
                .persist_messages(&[Message::user("recent"), Message::assistant("done", vec![])])
                .unwrap();
        }
        let budget = runtime_core::ContextBudget::new(10_000, Some(100), 0);
        assert!(state.open_session(&session, &budget).unwrap());
        let runtime = AgentKernel::new(
            Arc::new(MockProvider),
            ToolRegistry::new(),
            Arc::new(AllowAll),
        )
        .with_messages(state.loaded_messages.clone());
        let queue = runtime.task_queue().unwrap();
        assert_eq!(queue.overall_goal, "original goal");
        assert_eq!(
            queue.tasks[0].status,
            runtime_core::task_queue::TaskStatus::Completed
        );
        assert_eq!(
            queue.tasks[1].status,
            runtime_core::task_queue::TaskStatus::Running
        );
        state.runtime = Some(runtime);
        state.invalidate_runtime();
        let runtime = AgentKernel::new(
            Arc::new(MockProvider),
            ToolRegistry::new(),
            Arc::new(AllowAll),
        )
        .with_messages(state.loaded_messages.clone());
        assert_eq!(runtime.task_queue().unwrap().tasks[1].title, "two");
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    pub(crate) fn referenced_file_enters_runtime_and_persisted_session() {
        let root = std::env::temp_dir().join(format!("ax-file-context-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("README.md"), "Unique project instructions").unwrap();
        let mut state =
            ReplState::new_in_project(root.join(".ax"), root.join("skills"), None, &root).unwrap();
        state.ensure_session("check @README").unwrap();
        state.runtime = Some(AgentKernel::new(
            Arc::new(MockProvider),
            ToolRegistry::new(),
            Arc::new(AllowAll),
        ));
        state.prepare_file_context("check @README", 1_000).unwrap();
        assert!(
            state
                .runtime
                .as_ref()
                .unwrap()
                .messages()
                .iter()
                .any(|message| message.content.contains("Unique project instructions"))
        );
        let session = state.current_session_id().unwrap().to_owned();
        assert!(
            state
                .store()
                .unwrap()
                .load_messages(&session, None, 10)
                .unwrap()
                .iter()
                .any(|message| message.kind == MessageKind::AgentState
                    && message.content.contains("Unique project instructions"))
        );
        drop(state);
        fs::remove_dir_all(root).unwrap();
    }
}
