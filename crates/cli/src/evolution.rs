//! Observe the existing runtime event boundary; never alter its loop or tools.
use crate::{ModelSelection, ReplState};
use ::evolution::{Experience, Step};
use runtime_core::AgentEvent;
use std::collections::HashMap;

fn bounded(text: &str) -> String {
    text.chars().take(2048).collect()
}

pub(crate) struct Recorder {
    pub experience: Experience,
    calls: HashMap<String, usize>,
    sink: Option<::evolution::RecordSink>,
    completed: bool,
    instruction_paths: Vec<(String, String)>,
}
impl Recorder {
    pub fn new(state: &ReplState, prompt: &str, skills_used: Vec<String>) -> Self {
        Self {
            experience: Experience {
                id: uuid::Uuid::new_v4().to_string(),
                task: bounded(prompt),
                intent: bounded(prompt),
                tools_used: vec![],
                skills_used,
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
            sink: state.evolution.as_ref().and_then(::evolution::Handle::sink),
            completed: false,
            instruction_paths: state
                .skill_catalog
                .as_ref()
                .map(|catalog| {
                    catalog
                        .statuses(std::iter::empty::<&str>())
                        .into_iter()
                        .filter_map(|s| {
                            catalog
                                .instruction_path(&s.metadata.name)
                                .map(|p| (s.metadata.name, p.to_string_lossy().replace('\\', "/")))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
    pub fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::ToolStarted {
                id, name, detail, ..
            } => {
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
                if let Some(index) = self.calls.remove(id) {
                    self.experience.steps[index].success = Some(*success);
                    let detail = &self.experience.steps[index].detail;
                    if *success && name == "filesystem" && detail.starts_with("read ") {
                        let path = detail.trim_start_matches("read ").replace('\\', "/");
                        for (skill, instruction) in &self.instruction_paths {
                            if &path == instruction && !self.experience.skills_used.contains(skill)
                            {
                                self.experience.skills_used.push(skill.clone());
                            }
                        }
                    }
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
    let result = crate::run_prompt(state, selection, approval, prompt).await;
    state.evolution_finish().await;
    result.map(|_| ())
}

/// The kernel retains sole ownership of execution; this adapter observes its callback.
pub(crate) async fn checkpointed_turn<F>(
    runtime: &mut runtime_core::AgentKernel,
    state: &mut ReplState,
    prompt: &str,
    mut emit: F,
) -> Result<String, runtime_core::AgentError>
where
    F: FnMut(AgentEvent) + Send,
{
    let available = state.skill_tool_names();
    let skills_used = state
        .skill_catalog
        .as_ref()
        .expect("skill context prepared")
        .auto_route_candidates(prompt, available.iter().map(String::as_str))
        .into_iter()
        .filter(|s| state.active_skills.contains(&s.name))
        .map(|s| s.name)
        .collect();
    let mut recorder = Recorder::new(state, prompt, skills_used);
    let mut saved = 0;
    let intent = std::mem::take(&mut state.next_goal_turn);
    let result = runtime
        .run_goal_turn_checkpointed(
            prompt,
            intent,
            |event| {
                recorder.observe(&event);
                emit(event);
            },
            |messages| {
                state
                    .persist_turn_messages(messages, &mut saved)
                    .map_err(|error| runtime_core::AgentError::Persistence(error.to_string()))
            },
        )
        .await;
    recorder.experience.success = result.is_ok();
    if let Err(error) = &result {
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
            match crate::build_provider(selection, &crate::ax_auth_path()) {
                Ok(provider) => {
                    self.evolution = Some(::evolution::start(
                        self.evolution_root(),
                        crate::database_path(&self.data_dir),
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
                    !m.content.starts_with(crate::SKILL_CONTEXT_PREFIX)
                        || !m.content.contains(&prefix)
                });
                if let Some(runtime) = &mut self.runtime {
                    let stale: Vec<_> = runtime
                        .messages()
                        .iter()
                        .filter(|m| {
                            m.content.starts_with(crate::SKILL_CONTEXT_PREFIX)
                                && m.content.contains(&prefix)
                        })
                        .filter_map(crate::active_skill_name)
                        .collect();
                    for name in stale {
                        runtime
                            .set_context(&format!("{}{name}]", crate::SKILL_CONTEXT_PREFIX), None);
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
        let mut recorder = Recorder::new(f.state.as_ref().unwrap(), "build", vec![]);
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
        assert_eq!(messages.len(), 1);
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
                .all(|m| crate::active_skill_name(m).as_deref() != Some("sample-evolved"))
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
            crate::database_path(&state.data_dir),
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
        let recorder = Recorder::new(state, "build release", vec![]);
        drop(recorder); // Models/tasks can be cancelled by dropping their future.
        state.evolution_finish().await;
        let engine =
            ::evolution::Engine::open(root, engine.database, state.project_id.clone()).unwrap();
        assert_eq!(engine.ledger.experiences.len(), 1);
        assert!(!engine.ledger.experiences[0].success);
        assert!(engine.ledger.experiences[0].errors[0].contains("interrupted"));
    }
}
