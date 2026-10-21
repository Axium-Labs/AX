//! Observe the existing runtime event boundary; never alter its loop or tools.
use crate::{model_selection::ModelSelection, repl::ReplState};
use ::evolution::{Experience, Step};
use runtime_core::AgentEvent;
use std::collections::HashMap;

fn bounded(text: &str) -> String {
    text.chars().take(2048).collect()
}

pub(crate) struct Recorder {
    pub experience: Experience,
    calls: HashMap<String, usize>,
    skill_calls: HashMap<String, String>,
    sink: Option<::evolution::RecordSink>,
    completed: bool,
    instruction_paths: Vec<(String, String)>,
    root: std::path::PathBuf,
}
impl Recorder {
    pub fn new(state: &ReplState, prompt: &str) -> Self {
        Self {
            experience: Experience {
                id: uuid::Uuid::new_v4().to_string(),
                task: bounded(prompt),
                intent: String::new(), // Legacy deserialization only; never persisted.
                tools_used: vec![],
                skills_used: vec![],
                steps: vec![],
                errors: vec![],
                retries: 0,
                user_corrections: vec![],
                success: false,
                project: state.project_id.clone(),
                session: state
                    .current_session
                    .as_ref()
                    .expect("session initialized")
                    .id
                    .clone(),
                at: ::evolution::now(),
            },
            calls: HashMap::new(),
            skill_calls: HashMap::new(),
            sink: crate::config::AxConfig::load()
                .ok()
                .filter(|config| config.personalization.evolution_active())
                .and_then(|_| state.evolution.as_ref().and_then(::evolution::Handle::sink)),
            completed: false,
            instruction_paths: state
                .skill_catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .statuses(std::iter::empty::<&str>())
                        .into_iter()
                        .filter_map(|s| {
                            catalog.instruction_path(&s.metadata.name).map(|p| {
                                (s.metadata.name, normalized_path(&p, &state.project_root))
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            root: state.project_root.clone(),
        }
    }
    pub fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::ToolStarted {
                id,
                name,
                detail,
                input,
                ..
            } => {
                if name == "invoke_skill"
                    && let Some(skill) = input["name"].as_str()
                {
                    self.skill_calls.insert(id.clone(), skill.to_owned());
                }
                if name == "filesystem"
                    && input["operation"] == "read"
                    && let Some(path) = input["path"].as_str()
                {
                    let path = normalized_path(std::path::Path::new(path), &self.root);
                    if let Some((skill, _)) = self
                        .instruction_paths
                        .iter()
                        .find(|(_, instruction)| instruction == &path)
                    {
                        self.skill_calls.insert(id.clone(), skill.clone());
                    }
                }
                if !self.experience.tools_used.contains(name) {
                    self.experience.tools_used.push(name.clone());
                }
                if self.experience.steps.len() >= 64 {
                    return;
                }
                let detail = bounded(detail);
                if self.experience.steps.iter().any(|step| {
                    &step.tool == name && step.detail == detail && step.success == Some(false)
                }) {
                    self.experience.retries += 1;
                }
                self.calls.insert(id.clone(), self.experience.steps.len());
                self.experience.steps.push(Step {
                    tool: name.clone(),
                    detail,
                    success: None,
                });
            }
            AgentEvent::ToolFinished {
                id,
                name,
                success,
                diagnostics,
                ..
            } => {
                if let Some(skill) = self.skill_calls.remove(id)
                    && *success
                    && !self.experience.skills_used.contains(&skill)
                {
                    self.experience.skills_used.push(skill);
                }
                if let Some(index) = self.calls.remove(id) {
                    self.experience.steps[index].success = Some(*success);
                }
                if !success && self.experience.errors.len() < 64 {
                    self.experience.errors.push(format!("{name}: failed"));
                }
                for error in diagnostics
                    .iter()
                    .take(64 - self.experience.errors.len().min(64))
                {
                    self.experience
                        .errors
                        .push(bounded(&format!("{name}: {}", error.reason)));
                }
            }
            _ => {}
        }
    }
    pub fn complete(&mut self) {
        self.completed = true;
    }
}
fn normalized_path(path: &std::path::Path, root: &std::path::Path) -> String {
    let absolute = root.join(path);
    let absolute = absolute.canonicalize().unwrap_or(absolute);
    let mut normalized = std::path::PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            part => normalized.push(part.as_os_str()),
        }
    }
    normalized.to_string_lossy().replace('\\', "/")
}
impl Drop for Recorder {
    fn drop(&mut self) {
        if !self.completed {
            self.experience
                .errors
                .push("turn interrupted before completion".into());
        }
        if let Some(sink) = &self.sink {
            sink.record(self.experience.clone());
        }
    }
}

pub(crate) async fn run_once(
    state: &mut ReplState,
    selection: &ModelSelection,
    approval: std::sync::Arc<dyn runtime_core::ApprovalPolicy>,
    prompt: &str,
) -> anyhow::Result<()> {
    let result = crate::commands::run::run_prompt(state, selection, approval, prompt).await;
    state.evolution_finish().await;
    result.map(|_| ())
}

/// The kernel retains sole ownership of execution; this adapter observes its callback.
// One adapter orders ownership, raw persistence and Session activation.
#[allow(clippy::too_many_lines)]
pub(crate) async fn checkpointed_turn<F>(
    runtime: &mut runtime_core::AgentKernel,
    state: &mut ReplState,
    prompt: &str,
    emit: F,
) -> Result<String, runtime_core::AgentError>
where
    F: FnMut(AgentEvent) + Send,
{
    let project_config = match &state.next_goal_turn {
        runtime_core::GoalTurn::Pursue { config, .. } => Some(config),
        _ => runtime.goal().map(|g| &g.config),
    };
    if let Some(config) = project_config {
        crate::long_project::validate(config)
            .map_err(|e| runtime_core::AgentError::GoalMismatch(e.to_string()))?;
    }
    let project_enabled =
        project_config.is_some_and(|c| c.long_project.as_ref().is_some_and(|p| p.enabled));
    let project_host = if project_enabled {
        let owner = crate::long_project::owner(state)
            .map_err(|e| runtime_core::AgentError::GoalMismatch(e.to_string()))?;
        Some(crate::long_project::ProjectHost::new(
            state.project_root.clone(),
            state.current_session_id().unwrap_or_default().to_owned(),
            owner,
        ))
    } else {
        None
    };
    runtime.configure_long_project_host(
        project_host
            .clone()
            .map(|h| h as std::sync::Arc<dyn runtime_core::long_project::LongProjectHost>),
    );
    if let Some(host) = &project_host {
        *host.database.lock().unwrap() = Some(crate::bootstrap::database_path(&state.data_dir));
        runtime.register_tool(crate::long_project::ProjectTool(host.clone()));
    } else {
        runtime.remove_tool("long_project");
    }
    if std::mem::take(&mut state.next_rotation) {
        runtime.request_session_rotation();
    }
    let _goal_lease = if runtime.has_session_rotation()
        || runtime.goal().is_some()
        || matches!(state.next_goal_turn, runtime_core::GoalTurn::Pursue { .. })
        || state.current_session.as_ref().is_some_and(|session| {
            state
                .data_dir
                .join(format!("session-{}.lock", session.id))
                .exists()
        }) {
        Some(
            state
                .goal_session_lease(runtime.task_queue())
                .map_err(|e| runtime_core::AgentError::GoalMismatch(e.to_string()))?,
        )
    } else {
        None
    };
    let mut recorder = Recorder::new(state, prompt);
    let emit = std::sync::Mutex::new(emit);
    let mut saved = 0;
    let intent = std::mem::take(&mut state.next_goal_turn);
    let result = runtime
        .run_goal_turn_checkpointed(
            prompt,
            intent,
            |event| {
                recorder.observe(&event);
                (emit.lock().unwrap())(event);
            },
            |messages| {
                let previous_session = state.current_session_id().unwrap_or_default().to_owned();
                state
                    .persist_turn_messages(messages, &mut saved)
                    .map_err(|error| {
                        if messages.last().is_some_and(|m| {
                            m.content
                                .starts_with(runtime_core::long_project::ROTATION_PREFIX)
                        }) {
                            runtime_core::AgentError::GoalStopped {
                                state: runtime_core::GoalState::Paused,
                                reason: format!(
                                    "Session handoff failed; original history retained: {error}"
                                ),
                            }
                        } else {
                            runtime_core::AgentError::Persistence(error.to_string())
                        }
                    })?;
                if previous_session != state.current_session_id().unwrap_or_default() {
                    let goal_id = messages
                        .last()
                        .and_then(|m| {
                            m.content
                                .strip_prefix(runtime_core::long_project::ROTATION_PREFIX)
                        })
                        .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
                        .and_then(|v| v["goal_id"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    (emit.lock().unwrap())(AgentEvent::SessionRotated {
                        goal_id,
                        previous_session,
                        session: state.current_session_id().unwrap_or_default().to_owned(),
                    });
                }
                if let Some(host) = &project_host {
                    state
                        .current_session_id()
                        .unwrap_or_default()
                        .clone_into(&mut host.session.lock().unwrap());
                }
                Ok(())
            },
        )
        .await;
    state.rotation_leases.clear();
    recorder.experience.success =
        result.is_ok() || matches!(result, Err(runtime_core::AgentError::WaitingForUser(_)));
    if let Err(error) = &result
        && !matches!(error, runtime_core::AgentError::WaitingForUser(_))
    {
        recorder.experience.errors.push(bounded(&error.to_string()));
    }
    recorder.complete();
    result
}
impl ReplState {
    pub(crate) fn evolution_root(&self) -> std::path::PathBuf {
        self.data_dir.join("evolution").join(&self.project_id)
    }
    pub(crate) fn evolution_end_session(&self) {
        if let Some(handle) = &self.evolution {
            handle.end_session();
        }
    }
    pub(crate) async fn evolution_finish(&mut self) {
        if let Some(handle) = self.evolution.take() {
            handle.finish().await;
        }
    }
    pub(crate) fn evolution_prepare(&mut self, selection: &ModelSelection) {
        if !crate::config::AxConfig::load()
            .is_ok_and(|config| config.personalization.evolution_active())
        {
            return;
        }
        if self.evolution.is_none() {
            if let Err(error) = self.skills() {
                eprintln!("Evolution skill index: {error}");
                return;
            }
            // Indexing is already lazy and done for normal skill context on this turn.
            let protected = self
                .skill_catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .statuses(std::iter::empty::<&str>())
                        .iter()
                        .filter(|s| {
                            catalog
                                .directory(&s.metadata.name)
                                .is_some_and(|p| !p.starts_with(self.evolution_root()))
                        })
                        .map(|s| s.metadata.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            match crate::runtime::build_provider(selection, &crate::bootstrap::ax_auth_path()) {
                Ok(provider) => {
                    self.evolution = Some(::evolution::start(
                        self.evolution_root(),
                        crate::bootstrap::database_path(&self.data_dir),
                        self.project_id.clone(),
                        provider,
                        protected,
                    ));
                }
                Err(error) => eprintln!("Evolution unavailable: {error}"),
            }
        }
        if let Some(handle) = &self.evolution {
            let revision = handle.revision();
            if revision != self.evolution_revision {
                self.evolution_revision = revision;
                self.skill_catalog = None;
                self.capability_registries.borrow_mut().clear();
                // Refreshed evolved instructions will be routed again within the normal budget.
                let prefix = self.evolution_root().to_string_lossy().into_owned();
                self.loaded_messages.retain(|m| {
                    !m.content.starts_with(crate::repl::SKILL_CONTEXT_PREFIX)
                        || !m.content.contains(&prefix)
                });
                if let Some(runtime) = &mut self.runtime {
                    let stale: Vec<_> = runtime
                        .messages()
                        .iter()
                        .filter(|m| {
                            m.content.starts_with(crate::repl::SKILL_CONTEXT_PREFIX)
                                && m.content.contains(&prefix)
                        })
                        .filter_map(crate::repl::active_skill_name)
                        .collect();
                    for name in stale {
                        runtime.set_context(
                            &format!("{}{name}]", crate::repl::SKILL_CONTEXT_PREFIX),
                            None,
                        );
                        self.active_skills.remove(&name);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    struct Fixture {
        root: PathBuf,
        state: Option<ReplState>,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("ax-evolution-cli-{}", uuid::Uuid::new_v4()));
            let mut state =
                ReplState::new_in_project(root.join("data"), root.join("skills"), None, &root)
                    .unwrap();
            state.create_session("test").unwrap();
            state.allowed_skills = Some(std::collections::HashSet::from([
                "sample-evolved".into(),
                "sample-candidate".into(),
            ]));
            Self {
                root,
                state: Some(state),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.state.take();
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn recorder_preserves_completion_order_failures_and_retry_identity() {
        let f = Fixture::new();
        let mut recorder = Recorder::new(f.state.as_ref().unwrap(), "build");
        for (id, detail) in [("a", "cargo build"), ("b", "cargo test")] {
            recorder.observe(&AgentEvent::ToolStarted {
                id: id.into(),
                name: "shell".into(),
                detail: detail.into(),
                input: serde_json::Value::Null,
            });
        }
        for (id, success) in [("b", true), ("a", false)] {
            recorder.observe(&AgentEvent::ToolFinished {
                id: id.into(),
                name: "shell".into(),
                success,
                diagnostics: vec![],
                result: tool::ToolResult::new(success, String::new()),
            });
        }
        recorder.observe(&AgentEvent::ToolStarted {
            id: "c".into(),
            name: "shell".into(),
            detail: "cargo build".into(),
            input: serde_json::Value::Null,
        });
        assert_eq!(recorder.experience.steps[0].success, Some(false));
        assert_eq!(recorder.experience.steps[1].success, Some(true));
        assert_eq!(recorder.experience.retries, 1);
        assert_eq!(recorder.experience.tools_used, ["shell"]);
        assert_eq!(recorder.experience.errors, ["shell: failed"]);
        for _ in 0..100 {
            recorder.observe(&AgentEvent::ToolStarted {
                id: "z".into(),
                name: "shell".into(),
                detail: "many steps".into(),
                input: serde_json::Value::Null,
            });
        }
        assert_eq!(recorder.experience.steps.len(), 64);
    }

    #[test]
    fn candidates_do_not_route_user_packages_win_and_archived_context_is_not_resumed() {
        let mut f = Fixture::new();
        let state = f.state.as_mut().unwrap();
        let live = state.evolution_root().join("live");
        skill::create_skill_directory(
            &live,
            "sample-evolved",
            "Sample evolved release workflow",
            "Build carefully.",
        )
        .unwrap();
        skill::create_skill_directory(
            &state.evolution_root().join("candidate"),
            "sample-candidate",
            "Sample candidate workflow",
            "Trial later.",
        )
        .unwrap();
        assert!(
            state
                .skills()
                .unwrap()
                .directory("sample-candidate")
                .is_none()
        );
        let messages = state.route_skills("sample-evolved", 1000).unwrap();
        assert!(messages.is_empty());
        let loaded = state.skills().unwrap().load("sample-evolved").unwrap();
        let messages = vec![model::Message::system(format!(
            "[ax-skill:sample-evolved]\nSkill root: {}\n{}",
            loaded.directory.display(),
            loaded.instructions
        ))];
        state.persist_messages(&messages).unwrap();
        let session = state.current_session_id().unwrap().to_owned();
        let archived = state.evolution_root().join("archived");
        fs::create_dir_all(&archived).unwrap();
        fs::rename(live.join("sample-evolved"), archived.join("sample-evolved")).unwrap();
        state.skill_catalog = None;
        state.capability_registries.borrow_mut().clear();
        let budget = runtime_core::ContextBudget::new(32_000, Some(2048), 0);
        state.open_session(&session, &budget).unwrap();
        assert!(!state.active_skills.contains("sample-evolved"));
        assert!(
            state
                .loaded_messages
                .iter()
                .all(|m| crate::repl::active_skill_name(m).as_deref() != Some("sample-evolved"))
        );
        assert!(
            state
                .route_skills("sample-evolved", 1000)
                .unwrap()
                .is_empty()
        );
        let manual = skill::create_skill_directory(
            &state.skills_dir,
            "sample-evolved",
            "Sample explicit workflow",
            "Follow the user workflow.",
        )
        .unwrap();
        skill::create_skill_directory(
            &live,
            "sample-evolved",
            "Sample evolved workflow",
            "Follow the learned workflow.",
        )
        .unwrap();
        state.skill_catalog = None;
        state.capability_registries.borrow_mut().clear();
        assert_eq!(
            state.skills().unwrap().directory("sample-evolved"),
            Some(manual.as_path())
        );
    }

    #[test]
    fn shared_data_directory_does_not_share_learned_workflows_between_projects() {
        let mut f = Fixture::new();
        let first = f.state.as_mut().unwrap();
        let first_root = first.evolution_root();
        skill::create_skill_directory(
            &first_root.join("live"),
            "sample-evolved",
            "Sample evolved release workflow",
            "Only use in this project.",
        )
        .unwrap();
        let second_project = f.root.join("another-project");
        fs::create_dir_all(&second_project).unwrap();
        let mut second = ReplState::new_in_project(
            first.data_dir.clone(),
            second_project.join("skills"),
            None,
            &second_project,
        )
        .unwrap();
        assert_ne!(first.evolution_root(), second.evolution_root());
        assert!(
            second
                .skills()
                .unwrap()
                .directory("sample-evolved")
                .is_none()
        );
        drop(second);
    }

    struct NoAnalysis;
    #[async_trait::async_trait]
    impl model::ModelProvider for NoAnalysis {
        fn name(&self) -> &'static str {
            "test"
        }
        fn model_id(&self) -> &'static str {
            "test"
        }
        fn context_window(&self) -> usize {
            32_000
        }
        async fn complete(
            &self,
            _: model::ModelRequest,
        ) -> Result<model::ModelResponse, model::ModelError> {
            panic!("cooldown must prevent analysis")
        }
    }

    #[tokio::test]
    async fn cancelled_recorder_persists_a_failure_without_extra_model_requests() {
        let mut f = Fixture::new();
        let state = f.state.as_mut().unwrap();
        let root = state.evolution_root();
        let mut engine = ::evolution::Engine::open(
            root.clone(),
            crate::bootstrap::database_path(&state.data_dir),
            state.project_id.clone(),
        )
        .unwrap();
        engine.ledger.last_analysis = ::evolution::now();
        engine.save().unwrap();
        state.evolution = Some(::evolution::start(
            root.clone(),
            engine.database.clone(),
            state.project_id.clone(),
            std::sync::Arc::new(NoAnalysis),
            vec![],
        ));
        let recorder = Recorder::new(state, "build release");
        drop(recorder); // Models/tasks can be cancelled by dropping their future.
        state.evolution_finish().await;
        let engine =
            ::evolution::Engine::open(root, engine.database, state.project_id.clone()).unwrap();
        assert_eq!(engine.recent.len(), 1);
        assert!(!engine.recent[0].success);
        assert!(engine.recent[0].errors[0].contains("interrupted"));
    }
}
