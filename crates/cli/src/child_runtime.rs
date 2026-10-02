//! Persistent isolated child sessions, using the existing kernel and memory store.
use async_trait::async_trait;
use memory::{MemoryStore, MessageKind, NewMessage};
use model::Message;
use runtime_core::{
    AgentError, AgentKernel, ChildCheckpoint, ChildHost, ChildOutcome, ChildRun, PreparedChild,
};
use std::path::{Path, PathBuf};

const OUTCOME_PREFIX: &str = "[ax-child-outcome]\n";

pub(crate) struct LocalChildHost {
    pub sandbox: std::sync::OnceLock<Result<sandbox::SandboxManager, String>>,
    pub source: PathBuf,
    pub root: PathBuf,
    pub excluded: Vec<PathBuf>,
    pub policy: WorkspacePolicy,
}

/// Shared composition boundary for CLI, TUI and ACP prompt kernels.
/// No workspace is provisioned until the kernel requests a child.
pub(crate) fn configure_controller(state: &mut crate::repl::ReplState) -> anyhow::Result<()> {
    let host = std::sync::Arc::new(LocalChildHost::for_controller(
        &state.project_root,
        &state.data_dir,
    ));
    let runtime = state.runtime.take().expect("runtime initialized");
    state.runtime = Some(
        runtime
            .with_execution_scope(state.project_root.clone())
            .with_execution_budget(state.execution_budget)
            .with_child_host(host)
            .with_child_execution_budget(runtime_core::ExecutionBudget {
                turn_timeout_secs: state.child_timeout_secs,
                ..state.execution_budget
            }),
    );
    state.configure_scoped_subagents()
}

fn failure(error: impl std::fmt::Display) -> AgentError {
    AgentError::Persistence(error.to_string())
}

fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    let path = path.canonicalize()?;
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(path)
}

fn git(source: &Path, args: &[&std::ffi::OsStr]) -> std::io::Result<std::process::Output> {
    let source = absolute_path(source)?;
    let state = source.parent().unwrap_or(&source).join("state");
    let managed = workspace::read_manifest(&state).is_ok_and(|m| m.cwd == source);
    let mut policy = if managed {
        sandbox::SandboxManager::policy_for_child(source.clone(), &state)
    } else {
        sandbox::SandboxManager::policy_for_workspace(source.clone())
    }
    .map_err(std::io::Error::other)?;
    policy.mode = service_sandbox_mode();
    let first = args.first().and_then(|arg| arg.to_str());
    if first == Some("clone") {
        let destination = Path::new(
            args.last()
                .ok_or_else(|| std::io::Error::other("clone destination missing"))?,
        );
        let parent = destination
            .parent()
            .ok_or_else(|| std::io::Error::other("managed clone parent missing"))?;
        let manifest = workspace::read_manifest(&parent.join("state"))?;
        if manifest.cwd != destination {
            return Err(std::io::Error::other("invalid managed clone capability"));
        }
        policy.runtime_mounts.push(sandbox::RuntimeMount {
            path: parent.to_path_buf(),
            read_only: false,
        });
    }
    if managed && first == Some("apply") {
        policy.runtime_mounts.push(sandbox::RuntimeMount {
            path: state.clone(),
            read_only: true,
        });
    }
    if first == Some("worktree") && args.get(1).and_then(|arg| arg.to_str()) == Some("remove") {
        let destination = Path::new(args.last().unwrap());
        let parent = destination
            .parent()
            .ok_or_else(|| std::io::Error::other("managed worktree parent missing"))?;
        let manifest = workspace::read_manifest(&parent.join("state"))?;
        if manifest.cwd != destination || manifest.repository.as_ref() != Some(&source) {
            return Err(std::io::Error::other("invalid worktree cleanup capability"));
        }
        policy.runtime_mounts.push(sandbox::RuntimeMount {
            path: parent.to_path_buf(),
            read_only: false,
        });
    }
    // A policy carrying extra lifecycle capabilities needs its own manager; the
    // plain case can reuse the process-wide cached one.
    let runner = if policy.runtime_mounts.is_empty() {
        manager_for(&policy, &source, managed.then_some(state.as_path()))
    } else {
        sandbox::SandboxManager::prepare(&policy)
    }
    .map_err(std::io::Error::other)?;
    let mut spec = sandbox::CommandSpec::new("git");
    spec.args = vec![
        "-c".into(),
        "core.hooksPath=/dev/null".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-C".into(),
        source.to_string_lossy().into_owned(),
    ];
    spec.args
        .extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
    runner.output_blocking(spec).map_err(std::io::Error::other)
}

/// Reuses the cached manager when the requested mode is the process-wide one,
/// and otherwise prepares the caller's policy as its own manager. `child_state`
/// selects the managed-child capability when the caller has one.
pub(super) fn manager_for(
    policy: &sandbox::SandboxPolicy,
    source: &Path,
    child_state: Option<&Path>,
) -> sandbox::Result<sandbox::SandboxManager> {
    if sandbox::SandboxManager::configured_mode() == Some(policy.mode) {
        match child_state {
            Some(state) => {
                sandbox::SandboxManager::for_child_workspace(source.to_path_buf(), state)
            }
            None => sandbox::SandboxManager::for_workspace(source.to_path_buf()),
        }
    } else {
        sandbox::SandboxManager::prepare(policy)
    }
}
pub(super) fn service_sandbox_mode() -> sandbox::SandboxMode {
    // Falls back to the platform default so service calls, tests and interactive
    // runs resolve the same mode instead of three different ones.
    sandbox::SandboxManager::configured_mode().unwrap_or_default()
}

#[path = "child_workspace.rs"]
mod workspace;
pub(crate) use workspace::Policy as WorkspacePolicy;

struct SessionCheckpoint {
    database: PathBuf,
    session: String,
    saved: usize,
    state: PathBuf,
    root: PathBuf,
    policy: workspace::Policy,
    _lease: std::fs::File,
}
impl ChildCheckpoint for SessionCheckpoint {
    fn save(&mut self, messages: &[Message]) -> Result<(), AgentError> {
        let mut store = MemoryStore::open(&self.database).map_err(failure)?;
        for message in messages.iter().skip(self.saved) {
            store
                .append_message(
                    &self.session,
                    NewMessage {
                        role: crate::repl::memory_role(&message.role),
                        kind: if message.role == model::Role::System {
                            MessageKind::AgentState
                        } else if message.role == model::Role::Tool
                            || !message.tool_calls.is_empty()
                        {
                            MessageKind::ToolCall
                        } else {
                            MessageKind::Message
                        },
                        content: message.content.clone(),
                        metadata: serde_json::to_value(message).map_err(failure)?,
                    },
                )
                .map_err(failure)?;
            self.saved += 1;
        }
        drop(store);
        let mut manifest = workspace::read_manifest(&self.state).map_err(failure)?;
        manifest.touched = workspace::now();
        workspace::write_manifest(&self.state, &manifest).map_err(failure)?;
        workspace::check_quota(&self.root, &manifest.cwd, self.policy).map_err(failure)?;
        Ok(())
    }
    fn finish(&mut self, outcome: &ChildOutcome) -> Result<(), AgentError> {
        let mut store = MemoryStore::open(&self.database).map_err(failure)?;
        store
            .append_message(
                &self.session,
                NewMessage {
                    role: memory::MessageRole::System,
                    kind: MessageKind::AgentState,
                    content: format!(
                        "{OUTCOME_PREFIX}{}",
                        serde_json::to_string(outcome).map_err(failure)?
                    ),
                    metadata: serde_json::Value::Null,
                },
            )
            .map_err(failure)?;
        drop(store);
        let mut manifest = workspace::read_manifest(&self.state).map_err(failure)?;
        manifest.status = if outcome.success {
            "completed"
        } else {
            "failed"
        }
        .into();
        manifest.touched = workspace::now();
        workspace::write_manifest(&self.state, &manifest).map_err(failure)?;
        // Receipt is durable before workspace deletion; cleanup errors can be retried by GC.
        if let Err(error) = workspace::cleanup(&self.state) {
            eprintln!("child workspace cleanup deferred: {error}");
        }
        Ok(())
    }
}
impl Drop for SessionCheckpoint {
    fn drop(&mut self) {
        if let Ok(mut manifest) = workspace::read_manifest(&self.state)
            && manifest.status == "running"
        {
            manifest.status = "interrupted".into();
            manifest.touched = workspace::now();
            let _ = workspace::write_manifest(&self.state, &manifest);
        }
    }
}

type ChildStorage = (ChildRun, PathBuf, PathBuf, std::fs::File);

fn restore_saved(root: &Path, run: &ChildRun) -> Result<ChildStorage, AgentError> {
    let mut run = run.clone();
    // Legacy child stores move out of cwd before lifecycle cleanup is enabled.
    let parent = run
        .cwd
        .parent()
        .ok_or_else(|| failure("invalid saved child cwd"))?;
    if parent.parent() != Some(root) || run.cwd != parent.join("workspace") {
        return Err(failure("saved child workspace is outside child root"));
    }
    if absolute_path(parent).map_err(failure)?.parent() != Some(root) {
        return Err(failure(
            "saved child directory follows a link outside child root",
        ));
    }
    let state = parent.join("state");
    if state
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(failure("saved child state directory is a symlink"));
    }
    std::fs::create_dir_all(&state).map_err(failure)?;
    let lease = workspace::lease(&state).map_err(failure)?;
    if run.state_dir.is_none() {
        let legacy = run.cwd.join(".ax");
        if legacy.exists() {
            for entry in std::fs::read_dir(&legacy).map_err(failure)? {
                let entry = entry.map_err(failure)?;
                if matches!(
                    entry.file_name().to_str(),
                    Some("workspace.lock" | "workspace.json" | "workspace.json.tmp")
                ) {
                    continue;
                }
                std::fs::rename(entry.path(), state.join(entry.file_name())).map_err(failure)?;
            }
        }
        run.state_dir = Some(state.clone());
        let repository = git(
            &run.cwd,
            &[
                std::ffi::OsStr::new("rev-parse"),
                std::ffi::OsStr::new("--path-format=absolute"),
                std::ffi::OsStr::new("--git-common-dir"),
            ],
        )
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            PathBuf::from(String::from_utf8_lossy(&o.stdout).trim())
                .parent()
                .map(Path::to_path_buf)
        });
        workspace::write_manifest(
            &state,
            &workspace::Manifest {
                cwd: run.cwd.clone(),
                repository,
                status: "interrupted".into(),
                touched: workspace::now(),
            },
        )
        .map_err(failure)?;
    } else if run.state_dir.as_ref() != Some(&state) {
        return Err(failure("saved child state directory is outside child root"));
    }
    Ok((run, state.join("child.sqlite3"), state, lease))
}
impl LocalChildHost {
    fn for_controller(source: &Path, data_dir: &Path) -> Self {
        Self {
            sandbox: std::sync::OnceLock::new(),
            policy: WorkspacePolicy::default(),
            source: source.to_owned(),
            root: data_dir.join("child-runs"),
            excluded: vec![
                crate::bootstrap::database_path(data_dir),
                data_dir.join("sessions"),
                data_dir.join("evolution"),
            ],
        }
    }
    async fn provision_new(
        &self,
        input: &str,
        root: &Path,
        budget: runtime_core::ExecutionBudget,
    ) -> Result<ChildStorage, AgentError> {
        let policy = self.policy;
        let source = absolute_path(&self.source).map_err(failure)?;
        self.sandbox
            .get_or_init(|| {
                let mut policy = sandbox::SandboxManager::policy_for_workspace(source.clone())
                    .map_err(|e| e.to_string())?;
                policy.mode = service_sandbox_mode();
                manager_for(&policy, &source, None).map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(failure)?;
        let excluded = self
            .excluded
            .iter()
            .filter_map(|path| absolute_path(path).ok())
            .collect::<Vec<_>>();
        let provision_root = root.to_path_buf();
        let title = input.to_owned();
        let (cwd, state, database, session_id, lease) =
            tokio::task::spawn_blocking(move || -> Result<_, AgentError> {
                // Serialize admission/GC across controllers sharing this root.
                use fs2::FileExt;
                let admission = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(provision_root.join("admission.lock"))
                    .map_err(failure)?;
                admission.lock_exclusive().map_err(failure)?;
                workspace::gc(&provision_root, policy).map_err(failure)?;
                let available = policy
                    .total_bytes
                    .saturating_sub(workspace::usage(&provision_root).map_err(failure)?);
                if available == 0 {
                    return Err(failure("total child workspace disk quota exceeded"));
                }
                let provision_policy = workspace::Policy {
                    workspace_bytes: policy.workspace_bytes.min(available),
                    ..policy
                };
                let directory = provision_root.join(uuid::Uuid::new_v4().to_string());
                let state = directory.join("state");
                let cwd = directory.join("workspace");
                std::fs::create_dir_all(&state).map_err(failure)?;
                let lease = workspace::lease(&state).map_err(failure)?;
                workspace::write_manifest(
                    &state,
                    &workspace::Manifest {
                        cwd: cwd.clone(),
                        repository: None,
                        status: "running".into(),
                        touched: workspace::now(),
                    },
                )
                .map_err(failure)?;
                if let Err(error) = workspace::provision(
                    &source,
                    &cwd,
                    &provision_root,
                    &excluded,
                    &state,
                    provision_policy,
                ) {
                    let mut manifest = workspace::read_manifest(&state).map_err(failure)?;
                    manifest.status = "failed".into();
                    workspace::write_manifest(&state, &manifest).map_err(failure)?;
                    let _ = workspace::cleanup(&state);
                    return Err(AgentError::Tool(tool::ToolError::Execution(
                        error.to_string(),
                    )));
                }
                let database = state.join("child.sqlite3");
                let session =
                    MemoryStore::open(&database).and_then(|store| store.create_session(&title));
                match session {
                    Ok(session) => Ok((cwd, state, database, session.id, lease)),
                    Err(error) => {
                        let mut manifest = workspace::read_manifest(&state).map_err(failure)?;
                        manifest.status = "failed".into();
                        workspace::write_manifest(&state, &manifest).map_err(failure)?;
                        let _ = workspace::cleanup(&state);
                        Err(failure(error))
                    }
                }
            })
            .await
            .map_err(|e| AgentError::WorkerJoin(e.to_string()))??;
        let run = ChildRun {
            goal_id: format!("child-{session_id}"),
            memory_scope: format!("child:{session_id}"),
            execution_budget: Some(budget),
            session_id,
            cwd,
            state_dir: Some(state.clone()),
        };
        Ok((run, database, state, lease))
    }
}

#[async_trait]
impl ChildHost for LocalChildHost {
    async fn prepare_with_policy(
        &self,
        controller: &AgentKernel,
        input: &str,
        policy: &runtime_core::ChildPolicy,
    ) -> Result<PreparedChild, AgentError> {
        let mut child = self.prepare(controller, input, None).await?;
        if policy.workspace == runtime_core::child_policy::WorkspaceInheritance::Shared {
            // Keep disposable lifecycle paths intact; bind only the execution view to parent cwd.
            let mut binding = child.run.clone();
            binding.cwd.clone_from(&self.source);
            child.kernel = controller.fork_child(binding, input, child.kernel.messages().to_vec());
        }
        Ok(child)
    }

    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        previous: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError> {
        std::fs::create_dir_all(&self.root).map_err(failure)?;
        let root = absolute_path(&self.root).map_err(failure)?;
        let policy = self.policy;
        let (run, database, state, lease) = if let Some(run) = previous {
            restore_saved(&root, run)?
        } else {
            self.provision_new(input, &root, controller.child_execution_budget())
                .await?
        };
        let store = MemoryStore::open(&database).map_err(failure)?;
        if store.session(&run.session_id).map_err(failure)?.is_none() {
            return Err(failure("saved child session is missing"));
        }
        let mut terminal = store
            .latest_agent_state(&run.session_id, OUTCOME_PREFIX)
            .map_err(failure)?
            .map(|message| {
                serde_json::from_str(
                    message
                        .content
                        .strip_prefix(OUTCOME_PREFIX)
                        .unwrap_or_default(),
                )
                .map_err(failure)
            })
            .transpose()?;
        let messages = store
            .load_messages(&run.session_id, None, u32::MAX)
            .map_err(failure)?;
        let mut messages = messages
            .into_iter()
            .filter_map(|message| serde_json::from_value::<Message>(message.metadata).ok())
            .filter(|message| !message.content.starts_with("[ax-child-runtime]"))
            .collect::<Vec<_>>();
        // A final assistant message is already durable even if the process
        // disconnected between the final checkpoint and the terminal receipt.
        if terminal.is_none() {
            terminal = runtime_core::child::terminal_outcome(&messages);
        }
        drop(store);
        let mut manifest = workspace::read_manifest(&state).map_err(failure)?;
        if terminal.is_none() {
            let failure_reason = if manifest.status == "expired" || !run.cwd.is_dir() {
                Some(
                    "saved child workspace expired or missing; durable history retained".to_owned(),
                )
            } else {
                workspace::check_quota(&root, &run.cwd, policy)
                    .err()
                    .map(|error| error.to_string())
            };
            if let Some(output) = failure_reason {
                terminal = Some(ChildOutcome {
                    success: false,
                    output,
                });
            }
        }
        if let Some(outcome) = &terminal {
            // Includes a crash between durable final history and receipt creation.
            let mut checkpoint = SessionCheckpoint {
                database: database.clone(),
                session: run.session_id.clone(),
                saved: 0,
                state: state.clone(),
                root: root.clone(),
                policy,
                _lease: lease,
            };
            checkpoint.finish(outcome)?;
            return Ok(PreparedChild {
                kernel: controller.fork_child(run.clone(), input, vec![]),
                run,
                checkpoint: Box::new(checkpoint),
                terminal,
            });
        }
        manifest.status = "running".into();
        manifest.touched = workspace::now();
        workspace::write_manifest(&state, &manifest).map_err(failure)?;
        messages.insert(0, Message::system(format!("[ax-child-runtime]\n{}", serde_json::json!({
            "session_id":run.session_id,"cwd":run.cwd,"memory_scope":run.memory_scope,
            "state_dir":run.state_dir,"platform":std::env::consts::OS,"shell":if cfg!(windows) { "Windows PowerShell 5.1" } else { "POSIX sh" }
        }))));
        let kernel = controller.fork_child(run.clone(), input, messages);
        Ok(PreparedChild {
            checkpoint: Box::new(SessionCheckpoint {
                database,
                session: run.session_id.clone(),
                saved: 0,
                state,
                root,
                policy,
                _lease: lease,
            }),
            run,
            kernel,
            terminal,
        })
    }
}

#[cfg(test)]
#[path = "child_runtime_tests.rs"]
mod tests;
