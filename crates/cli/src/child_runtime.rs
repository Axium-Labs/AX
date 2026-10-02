//! Persistent isolated child sessions, using the existing kernel and memory store.
use async_trait::async_trait;
use memory::{MemoryStore, MessageKind, NewMessage};
use model::Message;
use runtime_core::{
    AgentError, AgentKernel, ChangedFile, ChildCheckpoint, ChildHost, ChildResult, ChildRun,
    ChildStatus, DiffStat, PreparedChild, from_durable_json,
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

/// Attach the operation to a provisioning error. A bare "access denied" in a
/// child receipt is not actionable; the path and the step are.
fn ctx<T>(label: &str, result: Result<T, impl std::fmt::Display>) -> Result<T, AgentError> {
    result.map_err(|error| failure(format!("{label}: {error}")))
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
    /// One connection for the checkpoint's lifetime. Reopening a store per save
    /// re-runs the schema migrations, which used to dominate the checkpoint
    /// cost; the connection is Send, so it can live inside the child's future.
    store: MemoryStore,
    session: String,
    saved: usize,
    state: PathBuf,
    root: PathBuf,
    /// The child's workspace, used to derive the authoritative diff.
    cwd: PathBuf,
    policy: workspace::Policy,
    /// Manifest and quota accounting are amortized: the GC TTL is minutes and
    /// the quota check re-walks the tree, so both run at most once a second.
    last_accounted: std::time::Instant,
    _lease: std::fs::File,
}
impl ChildCheckpoint for SessionCheckpoint {
    fn save(&mut self, messages: &[Message]) -> Result<(), AgentError> {
        let _timer = tool::telemetry::Timer::new("child.db_checkpoint");
        let mut batch = Vec::new();
        for message in messages.iter().skip(self.saved) {
            batch.push(NewMessage {
                role: crate::repl::memory_role(&message.role),
                kind: if message.role == model::Role::System {
                    MessageKind::AgentState
                } else if message.role == model::Role::Tool || !message.tool_calls.is_empty() {
                    MessageKind::ToolCall
                } else {
                    MessageKind::Message
                },
                content: message.content.clone(),
                metadata: serde_json::to_value(message).map_err(failure)?,
            });
        }
        self.saved += batch.len();
        // One transaction, one JSONL sync and one commit for the whole batch:
        // the fsync count of a save no longer scales with the message count.
        self.store
            .append_batch(&self.session, &batch)
            .map_err(failure)?;
        if self.last_accounted.elapsed() < std::time::Duration::from_secs(1) {
            return Ok(());
        }
        let mut manifest = workspace::read_manifest(&self.state).map_err(failure)?;
        manifest.touched = workspace::now();
        workspace::write_manifest(&self.state, &manifest).map_err(failure)?;
        workspace::check_cached_quota(&self.root, &manifest.cwd, self.policy).map_err(failure)?;
        self.last_accounted = std::time::Instant::now();
        Ok(())
    }
    fn finish(&mut self, result: &mut ChildResult) -> Result<(), AgentError> {
        // The workspace is the authority on what changed; the event-derived
        // list is only a fallback for a non-git child workspace. A read-only
        // child pays nothing: there is nothing to diff.
        if result.has_changes()
            && let Some((diff_stat, changed)) = workspace_diff(&self.cwd)
        {
            result.diff_stat = diff_stat;
            if !changed.is_empty() {
                result.changed_files.clone_from(&changed);
                result.artifacts = changed
                    .iter()
                    .map(|file| runtime_core::Artifact {
                        path: file.path.clone(),
                        kind: "workspace-file".into(),
                    })
                    .collect();
            }
        }
        self.store
            .append_message(
                &self.session,
                NewMessage {
                    role: memory::MessageRole::System,
                    kind: MessageKind::AgentState,
                    content: format!(
                        "{OUTCOME_PREFIX}{}",
                        serde_json::to_string(result).map_err(failure)?
                    ),
                    metadata: serde_json::Value::Null,
                },
            )
            .map_err(failure)?;
        let mut manifest = workspace::read_manifest(&self.state).map_err(failure)?;
        manifest.status = if result.status.success() {
            "completed"
        } else {
            "failed"
        }
        .into();
        manifest.touched = workspace::now();
        workspace::write_manifest(&self.state, &manifest).map_err(failure)?;
        let _admission = workspace::admission(&self.root);
        workspace::cleanup(&self.state).map_err(failure)?;
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

type ChildStorage = (ChildRun, MemoryStore, PathBuf, std::fs::File);

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
    let database = state.join("child.sqlite3");
    let store = MemoryStore::open(&database)
        .map_err(|error| failure(format!("{}: {error}", database.display())))?;
    Ok((run, store, state, lease))
}
/// Authoritative change set of a child workspace, read from git when the
/// workspace is one. Best effort: a non-git workspace simply keeps the
/// event-derived list.
fn workspace_diff(cwd: &Path) -> Option<(DiffStat, Vec<ChangedFile>)> {
    if !cwd.join(".git").exists() {
        return None;
    }
    let status = git(
        cwd,
        &[
            std::ffi::OsStr::new("status"),
            std::ffi::OsStr::new("--porcelain"),
        ],
    )
    .ok()?;
    if !status.status.success() {
        return None;
    }
    let mut changed = Vec::new();
    for line in String::from_utf8_lossy(&status.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let code = &line[..2];
        let path = line[2..].trim().trim_matches('"');
        if path.is_empty() {
            continue;
        }
        let change = if code.starts_with("??") || code.contains('A') {
            "created"
        } else if code.contains('D') {
            "deleted"
        } else {
            "modified"
        };
        changed.push(ChangedFile {
            path: path.to_owned(),
            change: change.to_owned(),
        });
    }
    changed.sort_by(|left, right| left.path.cmp(&right.path));
    changed.dedup_by(|left, right| left.path == right.path);
    let mut diff_stat = DiffStat {
        files: changed.len(),
        insertions: 0,
        deletions: 0,
    };
    if let Ok(numstat) = git(
        cwd,
        &[
            std::ffi::OsStr::new("diff"),
            std::ffi::OsStr::new("--numstat"),
            std::ffi::OsStr::new("HEAD"),
        ],
    ) && numstat.status.success()
    {
        for line in String::from_utf8_lossy(&numstat.stdout).lines() {
            let mut fields = line.split('\t');
            let (Some(added), Some(removed)) = (fields.next(), fields.next()) else {
                continue;
            };
            diff_stat.insertions += added.trim().parse::<u64>().unwrap_or(0);
            diff_stat.deletions += removed.trim().parse::<u64>().unwrap_or(0);
        }
    }
    Some((diff_stat, changed))
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
        let source = ctx("resolve workspace source", absolute_path(&self.source))?;
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
        let (cwd, state, store, session_id, lease) =
            tokio::task::spawn_blocking(move || -> Result<_, AgentError> {
                let used = ctx(
                    "child workspace usage",
                    workspace::cached_total_usage(&provision_root),
                )?;
                let available = policy.total_bytes.saturating_sub(used);
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
                ctx("create child state", std::fs::create_dir_all(&state))?;
                let lease = ctx("child state lease", workspace::lease(&state))?;
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
                let opened = MemoryStore::open(&database)
                    .and_then(|store| store.create_session(&title).map(|session| (store, session)));
                match opened {
                    Ok((store, session)) => Ok((cwd, state, store, session.id, lease)),
                    Err(error) => {
                        let mut manifest = workspace::read_manifest(&state).map_err(failure)?;
                        manifest.status = "failed".into();
                        workspace::write_manifest(&state, &manifest).map_err(failure)?;
                        let _ = workspace::cleanup(&state);
                        Err(failure(format!("{}: {error}", database.display())))
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
        Ok((run, store, state, lease))
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
        let _timer = tool::telemetry::Timer::new("child.startup");
        ctx("create child root", std::fs::create_dir_all(&self.root))?;
        let root = ctx("resolve child root", absolute_path(&self.root))?;
        let policy = self.policy;
        workspace::schedule_background_gc(&root, policy);
        let (run, store, state, lease) = if let Some(run) = previous {
            restore_saved(&root, run)?
        } else {
            self.provision_new(input, &root, controller.child_execution_budget())
                .await?
        };
        if store.session(&run.session_id).map_err(failure)?.is_none() {
            return Err(failure("saved child session is missing"));
        }
        let mut terminal = store
            .latest_agent_state(&run.session_id, OUTCOME_PREFIX)
            .map_err(failure)?
            .and_then(|message| {
                from_durable_json(
                    message
                        .content
                        .strip_prefix(OUTCOME_PREFIX)
                        .unwrap_or_default(),
                )
            });
        let messages = ctx(
            "load child messages",
            store.load_messages(&run.session_id, None, u32::MAX),
        )?;
        let mut messages = messages
            .into_iter()
            .filter_map(|message| serde_json::from_value::<Message>(message.metadata).ok())
            .filter(|message| !message.content.starts_with("[ax-child-runtime]"))
            .collect::<Vec<_>>();
        // A final assistant message is already durable even if the process
        // disconnected between the final checkpoint and the terminal receipt.
        if terminal.is_none() {
            terminal = runtime_core::child::terminal_result(&messages);
        }
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
                terminal = Some(ChildResult::failed(
                    String::new(),
                    String::new(),
                    ChildStatus::Failed,
                    output,
                ));
            }
        }
        if let Some(mut outcome) = terminal {
            // Includes a crash between durable final history and receipt creation.
            let mut checkpoint = SessionCheckpoint {
                store,
                session: run.session_id.clone(),
                saved: 0,
                state: state.clone(),
                root: root.clone(),
                cwd: run.cwd.clone(),
                policy,
                last_accounted: std::time::Instant::now(),
                _lease: lease,
            };
            checkpoint.finish(&mut outcome)?;
            return Ok(PreparedChild {
                kernel: controller.fork_child(run.clone(), input, vec![]),
                run,
                checkpoint: Box::new(checkpoint),
                terminal: Some(outcome),
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
                store,
                session: run.session_id.clone(),
                saved: 0,
                cwd: run.cwd.clone(),
                state,
                root,
                policy,
                last_accounted: std::time::Instant::now(),
                _lease: lease,
            }),
            run,
            kernel,
            terminal: None,
        })
    }
}

#[cfg(test)]
#[path = "child_runtime_tests.rs"]
mod tests;
