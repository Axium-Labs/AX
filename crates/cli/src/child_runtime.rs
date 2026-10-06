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
    let mut verification = crate::config::AxConfig::load()?.verification;
    let project_verification = state.project_root.join(".ax/verification.json");
    match std::fs::read(&project_verification) {
        Ok(bytes) => verification = serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for path in &mut verification.deliverables {
        if path.is_relative() {
            *path = state.project_root.join(&*path);
        }
    }
    state
        .runtime
        .as_mut()
        .unwrap()
        .configure_verification(verification);
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

#[path = "child_file_baseline.rs"]
mod file_baseline;
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
    output_dir: Option<PathBuf>,
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
        if self.cwd.is_dir() {
            if let Some((diff_stat, changed)) = workspace_diff(&self.cwd) {
                result.diff_stat = diff_stat;
                result.changed_files = changed;
            }
            freeze_artifacts(&self.cwd, &self.state, self.output_dir.as_deref(), result)?;
        } else {
            // A recovered terminal child has already been retired; preserve the
            // previously frozen receipt instead of overwriting its patch.
            if !self.state.join("result.json").is_file() {
                if result.status.success() {
                    return Err(failure("retired workspace has no durable receipt"));
                }
                durable_write(&self.state.join("final.patch"), b"")?;
                durable_write(
                    &self.state.join("result.json"),
                    &serde_json::to_vec_pretty(result).map_err(failure)?,
                )?;
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

fn durable_write(path: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    use std::io::Write;
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(failure(format!(
            "durable artifact is a symlink: {}",
            path.display()
        )));
    }
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut file = std::fs::File::create(&temporary).map_err(failure)?;
    file.write_all(bytes).map_err(failure)?;
    file.sync_all().map_err(failure)?;
    std::fs::rename(temporary, path).map_err(failure)
}

/// Freeze the answer and retain files/diagnostics before retiring the workspace.
fn capture_patch(
    cwd: &Path,
    state: &Path,
    result: &mut ChildResult,
) -> Result<Vec<u8>, AgentError> {
    use std::ffi::OsStr;
    Ok(if cwd.join(".git").exists() {
        // Intent-to-add includes new files in the final binary patch without
        // committing, changing HEAD, or touching the controller repository.
        let untracked = git(
            cwd,
            &[
                OsStr::new("ls-files"),
                OsStr::new("--others"),
                OsStr::new("--exclude-standard"),
                OsStr::new("-z"),
            ],
        )
        .map_err(failure)?;
        if !untracked.status.success() {
            return Err(failure(String::from_utf8_lossy(&untracked.stderr)));
        }
        for path in untracked
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
        {
            let path = String::from_utf8_lossy(path);
            if runtime_artifact(&path) {
                continue;
            }
            let add = git(
                cwd,
                &[
                    OsStr::new("add"),
                    OsStr::new("-N"),
                    OsStr::new("--"),
                    OsStr::new(path.as_ref()),
                ],
            )
            .map_err(failure)?;
            if !add.status.success() {
                return Err(failure(String::from_utf8_lossy(&add.stderr)));
            }
        }
        let diff = git(
            cwd,
            &[
                OsStr::new("diff"),
                OsStr::new("--binary"),
                OsStr::new("--full-index"),
                OsStr::new("HEAD"),
                OsStr::new("--"),
            ],
        )
        .map_err(failure)?;
        if !diff.status.success() {
            return Err(failure(String::from_utf8_lossy(&diff.stderr)));
        }
        if let Some((stat, changed)) = workspace_diff(cwd) {
            result.diff_stat = stat;
            result.changed_files = changed;
        }
        diff.stdout
    } else {
        file_baseline::apply(cwd, state, result)?;
        Vec::new()
    })
}

/// Staging is owned by the child; copy only regular files beneath its canonical
/// workspace and export through the host's validated output capability.
fn export_staged_artifacts(
    cwd: &Path,
    target: &Path,
) -> Result<Vec<runtime_core::Artifact>, AgentError> {
    let staging = cwd.join(".ax-artifacts");
    let mut artifacts = Vec::new();
    if !staging.is_dir() {
        return Ok(artifacts);
    }
    let root = absolute_path(cwd).map_err(failure)?;
    for entry in ignore::WalkBuilder::new(&staging).hidden(false).build() {
        let entry = entry.map_err(failure)?;
        if entry.file_type().is_some_and(|kind| kind.is_symlink()) {
            return Err(failure("staged artifact must not be a symlink"));
        }
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            let source = absolute_path(entry.path()).map_err(failure)?;
            if !source.starts_with(&root) {
                return Err(failure("staged artifact escapes workspace"));
            }
            let relative = entry.path().strip_prefix(&staging).map_err(failure)?;
            let destination = target.join(relative);
            std::fs::create_dir_all(destination.parent().unwrap()).map_err(failure)?;
            durable_write(&destination, &std::fs::read(source).map_err(failure)?)?;
            artifacts.push(runtime_core::Artifact {
                path: destination.to_string_lossy().into_owned(),
                kind: "output".into(),
            });
        }
    }
    Ok(artifacts)
}

fn freeze_artifacts(
    cwd: &Path,
    state: &Path,
    output: Option<&Path>,
    result: &mut ChildResult,
) -> Result<(), AgentError> {
    let patch = capture_patch(cwd, state, result)?;
    let targets = std::iter::once(state.to_path_buf())
        .chain(output.map(Path::to_path_buf))
        .collect::<Vec<_>>();
    let mut retained = Vec::new();
    let mut trace = Vec::new();
    for entry in &result.trace {
        trace.extend(serde_json::to_vec(entry).map_err(failure)?);
        trace.push(b'\n');
    }
    for target in &targets {
        std::fs::create_dir_all(target).map_err(failure)?;
        if target
            .symlink_metadata()
            .map_err(failure)?
            .file_type()
            .is_symlink()
        {
            return Err(failure("artifact output directory is a symlink"));
        }
        retained.extend(export_staged_artifacts(cwd, target)?);
        durable_write(&target.join("final.patch"), &patch)?;
        durable_write(&target.join("trace.jsonl"), &trace)?;
        retained.push(runtime_core::Artifact {
            path: target.join("final.patch").to_string_lossy().into_owned(),
            kind: "patch".into(),
        });
        for changed in &result.changed_files {
            let file = Path::new(&changed.path);
            if file.is_absolute()
                || file
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                return Err(failure("changed file escapes child workspace"));
            }
            if changed.change == "deleted" {
                continue;
            }
            let source = absolute_path(&cwd.join(file)).map_err(failure)?;
            if !source.starts_with(absolute_path(cwd).map_err(failure)?) {
                return Err(failure("changed artifact follows a link outside workspace"));
            }
            let destination = target.join("changed_files").join(file);
            std::fs::create_dir_all(destination.parent().unwrap()).map_err(failure)?;
            durable_write(&destination, &std::fs::read(source).map_err(failure)?)?;
            retained.push(runtime_core::Artifact {
                path: destination.to_string_lossy().into_owned(),
                kind: "changed-file".into(),
            });
        }
        // Preserve declared relative artifacts as well as the diff. Durable
        // references must never point at a deleted workspace.
        for artifact in &result.artifacts {
            let source = if Path::new(&artifact.path).is_absolute() {
                PathBuf::from(&artifact.path)
            } else {
                cwd.join(&artifact.path)
            };
            if source.is_file()
                && absolute_path(&source)
                    .map_err(failure)?
                    .starts_with(absolute_path(cwd).map_err(failure)?)
            {
                let relative = source.strip_prefix(cwd).map_err(failure)?;
                let destination = target.join("artifacts").join(relative);
                std::fs::create_dir_all(destination.parent().unwrap()).map_err(failure)?;
                durable_write(&destination, &std::fs::read(source).map_err(failure)?)?;
                retained.push(runtime_core::Artifact {
                    path: destination.to_string_lossy().into_owned(),
                    kind: artifact.kind.clone(),
                });
            }
        }
    }
    result.artifacts.retain(|artifact| {
        Path::new(&artifact.path).is_absolute() && !Path::new(&artifact.path).starts_with(cwd)
    });
    result.artifacts.extend(retained);
    write_frozen_receipts(
        &targets,
        state,
        cwd.join(".ax-artifacts/result.json").is_file(),
        result,
    )
}

fn write_frozen_receipts(
    targets: &[PathBuf],
    state: &Path,
    custom_result: bool,
    result: &mut ChildResult,
) -> Result<(), AgentError> {
    for target in targets {
        for name in [
            "result.json",
            "child_result.json",
            "trace.jsonl",
            "final.patch",
            "metrics.json",
            "validation.json",
            "diagnostics.json",
            "artifact-manifest.json",
        ] {
            let path = target.join(name).to_string_lossy().into_owned();
            if !result
                .artifacts
                .iter()
                .any(|artifact| artifact.path == path)
            {
                result.artifacts.push(runtime_core::Artifact {
                    path,
                    kind: "host-output".into(),
                });
            }
        }
    }
    for target in targets {
        // The manifest identifies current exports without deleting previous user
        // outputs. Consumers must not treat unlisted custom files as this run's evidence.
        let mut current = result
            .artifacts
            .iter()
            .filter(|artifact| Path::new(&artifact.path).starts_with(target))
            .cloned()
            .collect::<Vec<_>>();
        for name in [
            "result.json",
            "child_result.json",
            "trace.jsonl",
            "final.patch",
            "metrics.json",
            "validation.json",
            "diagnostics.json",
        ] {
            let path = target.join(name).to_string_lossy().into_owned();
            if !current.iter().any(|artifact| artifact.path == path) {
                current.push(runtime_core::Artifact {
                    path,
                    kind: "host-output".into(),
                });
            }
        }
        durable_write(
            &target.join("artifact-manifest.json"),
            &serde_json::to_vec_pretty(&serde_json::json!({
                "child_id": result.child_id,
                "task_id": result.task_id,
                "status": result.status,
                "artifacts": current,
                "authoritative_receipt": "child_result.json",
                "metrics": "metrics.json",
                "instruction": "Only listed custom artifacts belong to this execution. Unlisted files may be from earlier runs; unavailable evaluation is null. Custom result status is descriptive; use the authoritative receipt for terminal status."
            })).map_err(failure)?,
        )?;
        durable_write(
            &target.join("child_result.json"),
            &serde_json::to_vec_pretty(result).map_err(failure)?,
        )?;
        if target.as_path() == state || !custom_result {
            durable_write(
                &target.join("result.json"),
                &serde_json::to_vec_pretty(result).map_err(failure)?,
            )?;
        }
        durable_write(
            &target.join("validation.json"),
            &serde_json::to_vec_pretty(&result.validation).map_err(failure)?,
        )?;
        durable_write(
            &target.join("metrics.json"),
            &serde_json::to_vec_pretty(&result.metrics).map_err(failure)?,
        )?;
        durable_write(
            &target.join("diagnostics.json"),
            &serde_json::to_vec_pretty(&result.diagnostics).map_err(failure)?,
        )?;
    }
    Ok(())
}

fn restore_saved(root: &Path, run: &ChildRun) -> Result<ChildStorage, AgentError> {
    let mut run = run.clone();
    // Legacy child stores move out of cwd before lifecycle cleanup is enabled.
    let parent = run
        .workspace_root()
        .parent()
        .ok_or_else(|| failure("invalid saved child cwd"))?;
    if parent.parent() != Some(root) || run.workspace_root() != parent.join("workspace") {
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
        let legacy = run.workspace_root().join(".ax");
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
                cwd: run.workspace_root().to_path_buf(),
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
            std::ffi::OsStr::new("--porcelain=v1"),
            std::ffi::OsStr::new("-z"),
            std::ffi::OsStr::new("--untracked-files=all"),
        ],
    )
    .ok()?;
    if !status.status.success() {
        return None;
    }
    let mut changed = Vec::new();
    let mut records = status.stdout.split(|byte| *byte == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let code = String::from_utf8_lossy(&record[..2]);
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        if code.contains('R') || code.contains('C') {
            let _ = records.next();
        }
        if code == "??" && runtime_artifact(&path) {
            continue;
        }
        let change = if code == "??" || code.contains('A') {
            "created"
        } else if code.contains('D') {
            "deleted"
        } else {
            "modified"
        };
        changed.push(ChangedFile {
            path,
            change: change.into(),
        });
    }
    changed.sort_by(|left, right| left.path.cmp(&right.path));
    changed.dedup_by(|left, right| left.path == right.path);
    let mut diff_stat = DiffStat {
        files: changed.len(),
        insertions: None,
        deletions: None,
    };
    if let Ok(numstat) = git(
        cwd,
        &[
            std::ffi::OsStr::new("diff"),
            std::ffi::OsStr::new("--numstat"),
            std::ffi::OsStr::new("-z"),
            std::ffi::OsStr::new("HEAD"),
        ],
    ) && numstat.status.success()
    {
        let mut insertions = 0_u64;
        let mut deletions = 0_u64;
        let mut known = true;
        let mut records = numstat.stdout.split(|byte| *byte == 0);
        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }
            let fields = record.splitn(3, |byte| *byte == b'\t').collect::<Vec<_>>();
            if fields.len() != 3 {
                known = false;
                continue;
            }
            if fields[2].is_empty() {
                let _ = records.next();
                let _ = records.next();
            }
            match (
                std::str::from_utf8(fields[0])
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok()),
                std::str::from_utf8(fields[1])
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok()),
            ) {
                (Some(added), Some(removed)) => {
                    insertions = insertions.saturating_add(added);
                    deletions = deletions.saturating_add(removed);
                }
                _ => known = false,
            }
        }
        diff_stat.insertions = known.then_some(insertions);
        diff_stat.deletions = known.then_some(deletions);
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
        spec: runtime_core::task_queue::WorkspaceSpec,
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
                if let Err(error) = workspace::provision_spec(
                    &source,
                    &cwd,
                    &provision_root,
                    &excluded,
                    &state,
                    provision_policy,
                    &spec,
                ) {
                    let mut manifest = workspace::read_manifest(&state).map_err(failure)?;
                    manifest.status = "failed".into();
                    workspace::write_manifest(&state, &manifest).map_err(failure)?;
                    let _ = workspace::cleanup(&state);
                    return Err(AgentError::Tool(tool::ToolError::Execution(
                        error.to_string(),
                    )));
                }
                file_baseline::save(&cwd, &state)?;
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
            workspace_root: None,
            cwd,
            state_dir: Some(state.clone()),
        };
        Ok((run, store, state, lease))
    }
}

#[async_trait]
impl ChildHost for LocalChildHost {
    fn persist_preparation_failure(
        &self,
        task: &runtime_core::task_queue::QueuedTask,
        result: &mut ChildResult,
    ) -> Result<(), AgentError> {
        let directory = self
            .root
            .join("setup-failures")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&directory).map_err(failure)?;
        result.artifacts.push(runtime_core::Artifact {
            path: directory.join("result.json").to_string_lossy().into_owned(),
            kind: "setup-receipt".into(),
        });
        durable_write(
            &directory.join("result.json"),
            &serde_json::to_vec_pretty(result).map_err(failure)?,
        )?;
        if let Some(output) = task.output_dir.as_deref() {
            let output = self.source.join(output);
            validate_output(&self.source, &output)?;
            std::fs::create_dir_all(&output).map_err(failure)?;
            durable_write(&output.join("final.patch"), b"")?;
            durable_write(&output.join("trace.jsonl"), b"")?;
            write_frozen_receipts(&[output], &directory, false, result)?;
        }
        Ok(())
    }

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

    async fn prepare_task(
        &self,
        controller: &AgentKernel,
        task: &runtime_core::task_queue::QueuedTask,
    ) -> Result<PreparedChild, AgentError> {
        self.prepare_spec(
            controller,
            task.task_input(),
            task.child.as_ref(),
            task.workspace.clone(),
            task.output_dir.as_deref().map(PathBuf::from),
        )
        .await
    }
    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        previous: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError> {
        self.prepare_spec(
            controller,
            input,
            previous,
            runtime_core::task_queue::WorkspaceSpec::default(),
            None,
        )
        .await
    }
}

impl LocalChildHost {
    // Keep restore, terminal receipt and new-run binding in one lifecycle transaction.
    #[allow(clippy::too_many_lines)]
    async fn prepare_spec(
        &self,
        controller: &AgentKernel,
        input: &str,
        previous: Option<&ChildRun>,
        spec: runtime_core::task_queue::WorkspaceSpec,
        output_dir: Option<PathBuf>,
    ) -> Result<PreparedChild, AgentError> {
        let output_dir = output_dir.map(|path| self.source.join(path));
        if let Some(output) = &output_dir {
            validate_output(&self.source, output)?;
        }
        let _timer = tool::telemetry::Timer::new("child.startup");
        ctx("create child root", std::fs::create_dir_all(&self.root))?;
        let root = ctx("resolve child root", absolute_path(&self.root))?;
        let policy = self.policy;
        workspace::schedule_background_gc(&root, policy);
        let (mut run, store, state, lease) = if let Some(run) = previous {
            restore_saved(&root, run)?
        } else {
            self.provision_new(
                input,
                &root,
                controller.child_execution_budget(),
                spec.clone(),
            )
            .await?
        };
        let lifecycle_root = run.workspace_root().to_path_buf();
        if previous.is_none()
            && let Some(subdir) = &spec.subdir
        {
            let cwd = absolute_path(&lifecycle_root.join(subdir)).map_err(failure)?;
            if !cwd.starts_with(absolute_path(&lifecycle_root).map_err(failure)?) {
                return Err(failure("subdir escapes child workspace"));
            }
            run.workspace_root = Some(lifecycle_root.clone());
            run.cwd = cwd;
        }
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
        if let Some(output) = &output_dir {
            durable_write(
                &state.join("output-dir.json"),
                &serde_json::to_vec(&serde_json::json!({"directory":output,"source":self.source}))
                    .map_err(failure)?,
            )?;
        }
        let mut manifest = workspace::read_manifest(&state).map_err(failure)?;
        if terminal.is_none() {
            let failure_reason = if manifest.status == "expired" || !run.cwd.is_dir() {
                Some(
                    "saved child workspace expired or missing; durable history retained".to_owned(),
                )
            } else {
                workspace::check_quota(&root, &lifecycle_root, policy)
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
                cwd: lifecycle_root.clone(),
                output_dir: output_dir.clone(),
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
        if let Some(output) = &output_dir {
            std::fs::create_dir_all(lifecycle_root.join(".ax-artifacts")).map_err(failure)?;
            messages.push(Message::system(format!(
                "[ax-artifact-output]\nDurable output: {}. Write requested report/JSON/evaluation files under staging directory {} (inside the workspace); the host exports these files to the durable output root before cleanup. Keep staging files out of the coding patch. The host freezes final.patch and measured metrics/trace; unavailable evaluation values stay null.", output.display(), lifecycle_root.join(".ax-artifacts").display()
            )));
        }
        messages.retain(|message| !message.content.starts_with("[ax-task-workspace]"));
        messages.push(Message::system(format!("[ax-task-workspace]\n{}", serde_json::json!({
            "workspace":spec,"cwd":run.cwd,"workspace_root":lifecycle_root,
            "prepared":true,
            "instruction":if spec.mode == runtime_core::task_queue::WorkspaceMode::Git {
                "The host has already prepared this repository at the requested exact revision. Edit and validate the current checkout at cwd. Do not clone another copy or switch revisions; the host captures this workspace's diff before cleanup."
            } else {
                "The host has prepared this task's isolated workspace. Work at cwd with the inputs supplied to this task; an empty workspace does not contain controller or sibling output directories."
            }
        }))));
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
                cwd: lifecycle_root,
                output_dir,
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

#[cfg(test)]
#[path = "../../../test/harness/workspaces.rs"]
mod harness_workspace_tests;

fn validate_output(source: &Path, output: &Path) -> Result<(), AgentError> {
    if !output.is_absolute()
        || output
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(failure(
            "output_dir must be an absolute non-traversing path",
        ));
    }
    let policy =
        sandbox::SandboxManager::policy_for_workspace(source.to_path_buf()).map_err(failure)?;
    let mut ancestor = output;
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| failure("invalid output directory"))?;
    }
    let resolved = absolute_path(ancestor)
        .map_err(failure)?
        .join(output.strip_prefix(ancestor).map_err(failure)?);
    if policy
        .protected_paths
        .iter()
        .any(|protected| resolved.starts_with(protected))
        || (service_sandbox_mode() != sandbox::SandboxMode::Off
            && !resolved.starts_with(absolute_path(source).map_err(failure)?))
    {
        return Err(failure("output_dir is outside permitted write boundary"));
    }
    Ok(())
}

fn runtime_artifact(path: &str) -> bool {
    Path::new(path).components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some(
                ".venv"
                    | ".ax-artifacts"
                    | "venv"
                    | ".ax"
                    | "node_modules"
                    | "target"
                    | "__pycache__"
                    | ".pytest_cache"
            )
        )
    })
}
