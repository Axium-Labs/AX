//! Disposable child workspaces: Git overlays, filtered snapshots and bounded GC.
use super::{absolute_path, git};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy)]
pub(crate) struct Policy {
    pub workspace_bytes: u64,
    pub total_bytes: u64,
    pub ttl_secs: u64,
}
impl Default for Policy {
    fn default() -> Self {
        fn value(name: &str, default: u64) -> u64 {
            std::env::var(name)
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(default)
        }
        Self {
            workspace_bytes: value("AX_CHILD_WORKSPACE_QUOTA_BYTES", 2 * 1024 * 1024 * 1024),
            total_bytes: value("AX_CHILD_TOTAL_QUOTA_BYTES", 8 * 1024 * 1024 * 1024),
            ttl_secs: value("AX_CHILD_WORKSPACE_TTL_SECS", 7 * 24 * 3600),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Manifest {
    pub cwd: PathBuf,
    pub repository: Option<PathBuf>,
    pub status: String,
    pub touched: u64,
}
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(super) fn read_manifest(state: &Path) -> io::Result<Manifest> {
    serde_json::from_slice(&std::fs::read(state.join("workspace.json"))?).map_err(io::Error::other)
}
pub(super) fn write_manifest(state: &Path, manifest: &Manifest) -> io::Result<()> {
    let temporary = state.join("workspace.json.tmp");
    let mut file = File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(manifest)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, state.join("workspace.json"))
}
pub(super) fn lease(state: &Path) -> io::Result<File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("workspace.lock"))?;
    file.try_lock_exclusive()?;
    Ok(file)
}
fn safe_workspace(state: &Path, cwd: &Path) -> io::Result<()> {
    // Reject corrupt/traversing manifests, symlinked parent directories and roots.
    let parent = state
        .parent()
        .ok_or_else(|| io::Error::other("invalid child state directory"))?;
    if cwd != parent.join("workspace")
        || cwd
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(io::Error::other(
            "child workspace outside owned child directory",
        ));
    }
    if cwd.exists() && !absolute_path(cwd)?.starts_with(absolute_path(parent)?) {
        return Err(io::Error::other(
            "child workspace link escapes owned directory",
        ));
    }
    Ok(())
}
pub(super) fn cleanup(state: &Path) -> io::Result<()> {
    let manifest = read_manifest(state)?;
    safe_workspace(state, &manifest.cwd)?;
    let scratch = state.join("input.patch");
    if scratch.exists() {
        std::fs::remove_file(scratch)?;
    }
    if manifest.cwd.exists() {
        if let Some(repository) = &manifest.repository {
            let result = git(
                repository,
                &[
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    OsStr::new("--force"),
                    manifest.cwd.as_os_str(),
                ],
            )?;
            if !result.status.success() {
                return Err(io::Error::other(format!(
                    "worktree cleanup failed: {}",
                    String::from_utf8_lossy(&result.stderr)
                )));
            }
        } else {
            std::fs::remove_dir_all(&manifest.cwd)?;
        }
    }
    if let Some(root) = state.parent().and_then(Path::parent) {
        invalidate_usage_cache(root);
    }
    Ok(())
}
/// Admission guard for one child root: provisioning, quota accounting and
/// cleanup run one at a time, so a workspace tree is never scanned while
/// another child is being deleted. On Windows a delete-pending directory reads
/// as `access denied`, which used to fail an unrelated child's provisioning.
///
/// The in-process lock is authoritative. Every child of a controller runs in
/// that controller's process, and Windows refuses a second exclusive file lock
/// taken from the same process instead of blocking on it. The file lock is
/// still taken, best effort, so a second AX process sharing the same data
/// directory serializes with us; if it stays busy, the child proceeds under the
/// process lock rather than failing.
pub(super) struct Admission {
    _local: std::sync::MutexGuard<'static, ()>,
    _file: Option<std::fs::File>,
}

pub(super) fn admission(root: &Path) -> Admission {
    let local = local_lock(root)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("admission.lock"))
        .ok()
        .filter(cross_process_lock);
    Admission {
        _local: local,
        _file: file,
    }
}

/// One lock per child root, kept for the process lifetime. The set is bounded
/// by the number of project data directories this process serves.
fn local_lock(root: &Path) -> &'static std::sync::Mutex<()> {
    static REGISTRY: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, &'static std::sync::Mutex<()>>>,
    > = std::sync::OnceLock::new();
    let registry = REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut locks = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks
        .entry(root.to_path_buf())
        .or_insert_with(|| Box::leak(Box::new(std::sync::Mutex::new(()))))
}

fn cross_process_lock(file: &std::fs::File) -> bool {
    use fs2::FileExt;
    for _ in 0..200 {
        match file.try_lock_exclusive() {
            Ok(()) => return true,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => return false,
        }
    }
    false
}

pub(super) fn schedule_background_gc(root: &Path, policy: Policy) {
    static SCHEDULED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    let scheduled =
        SCHEDULED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let mut guard = scheduled
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !guard.insert(root.to_path_buf()) {
        return;
    }
    drop(guard);
    let gc_root = root.to_path_buf();
    std::thread::spawn(move || {
        let _admission = admission(&gc_root);
        if let Err(error) = gc(&gc_root, policy) {
            eprintln!("child workspace GC deferred: {error}");
        }
    });
}

pub(super) fn gc(root: &Path, policy: Policy) -> io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let Ok(entry) = entry else {
            // A concurrent cleanup can retire an entry between read and use.
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let state = entry.path().join("state");
        let state_is_dir = state.is_dir()
            && state
                .symlink_metadata()
                .is_ok_and(|meta| !meta.file_type().is_symlink());
        if !state_is_dir {
            continue;
        }
        let Ok(_lease) = lease(&state) else {
            continue;
        };
        let Ok(mut manifest) = read_manifest(&state) else {
            // A child directory without a readable manifest is an orphan from an
            // interrupted provisioning step: it has no owner, no receipt and no
            // resume path, so it is reclaimed on the same idle TTL instead of
            // being skipped forever.
            if now().saturating_sub(modified_secs(&entry.path())) >= policy.ttl_secs
                && let Err(error) = std::fs::remove_dir_all(entry.path())
            {
                eprintln!("child workspace GC deferred: {error}");
            }
            continue;
        };
        let terminal = matches!(manifest.status.as_str(), "completed" | "failed" | "expired");
        if terminal || now().saturating_sub(manifest.touched) >= policy.ttl_secs {
            if manifest.cwd.is_dir() && !state.join("result.json").is_file() {
                let mut result = runtime_core::ChildResult::failed(
                    "expired",
                    "expired",
                    runtime_core::ChildStatus::Failed,
                    "unleased child workspace expired",
                );
                let saved = std::fs::read(state.join("output-dir.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
                let output = saved
                    .as_ref()
                    .and_then(|saved| saved["directory"].as_str().or_else(|| saved.as_str()))
                    .map(PathBuf::from);
                let source = saved
                    .as_ref()
                    .and_then(|saved| saved["source"].as_str())
                    .map_or_else(|| root.to_path_buf(), PathBuf::from);
                if let Some(path) = &output {
                    super::validate_output(&source, path).map_err(io::Error::other)?;
                }
                if let Err(error) =
                    super::freeze_artifacts(&manifest.cwd, &state, output.as_deref(), &mut result)
                {
                    eprintln!("child artifact preservation deferred: {error}");
                    continue;
                }
            }
            if let Err(error) = cleanup(&state) {
                // A locked/corrupt old workspace must not fail every new child.
                eprintln!("child workspace GC deferred: {error}");
                continue;
            }
            if !terminal {
                manifest.status = "expired".into();
                write_manifest(&state, &manifest)?;
            }
        }
    }
    Ok(())
}

/// Disposable bytes under one directory. A directory that disappears mid-scan
/// reports what was measured so far: quota accounting is an estimate, never a
/// reason for an unrelated child to fail.
pub(super) fn size(path: &Path) -> io::Result<u64> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Ok(0);
    };
    let mut bytes = 0_u64;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        bytes = bytes.saturating_add(if kind.is_dir() {
            size(&entry.path())?
        } else {
            entry.metadata().map_or(0, |meta| meta.len())
        });
    }
    Ok(bytes)
}
pub(super) fn check_quota(root: &Path, cwd: &Path, policy: Policy) -> io::Result<()> {
    check_quota_inner(root, cwd, policy, true)
}

pub(super) fn check_cached_quota(root: &Path, cwd: &Path, policy: Policy) -> io::Result<()> {
    check_quota_inner(root, cwd, policy, false)
}

fn check_quota_inner(root: &Path, cwd: &Path, policy: Policy, exact_total: bool) -> io::Result<()> {
    let _timer = tool::telemetry::Timer::new("child.quota_scan");
    if size(cwd)? > policy.workspace_bytes {
        return Err(io::Error::other("child workspace disk quota exceeded"));
    }
    let total = if exact_total {
        usage(root)?
    } else {
        cached_total_usage(root)?
    };
    if total > policy.total_bytes {
        return Err(io::Error::other(
            "total child workspace disk quota exceeded",
        ));
    }
    Ok(())
}
/// Total disposable bytes owned by every child directory. The durable `state`
/// directory is deliberately excluded: it holds retained history that outlives
/// the workspace, and `SQLite`'s minimum page size alone would dwarf a small
/// configured workspace quota.
/// Whole-tree usage, refreshed at most every two seconds per root. The
/// current child's own workspace is always measured exactly (`check_quota`
/// walks it directly); the *siblings'* totals are the part that used to be
/// re-scanned on every checkpoint, and a two-second-old estimate of those is
/// accurate enough for a safety bound.
pub(super) fn cached_total_usage(root: &Path) -> io::Result<u64> {
    const TTL: std::time::Duration = std::time::Duration::from_secs(2);
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::Instant, u64)>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    {
        let guard = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, bytes)) = guard.get(root)
            && at.elapsed() < TTL
        {
            return Ok(*bytes);
        }
    }
    let bytes = usage(root)?;
    let mut guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.len() >= 64 {
        guard.retain(|_, (at, _)| at.elapsed() < TTL);
    }
    guard.insert(root.to_path_buf(), (std::time::Instant::now(), bytes));
    Ok(bytes)
}

pub(super) fn invalidate_usage_cache(root: &Path) {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::Instant, u64)>>,
    > = std::sync::OnceLock::new();
    if let Some(cache) = CACHE.get() {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(root);
    }
}

pub(super) fn usage(root: &Path) -> io::Result<u64> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(0);
    };
    let mut bytes = 0_u64;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && !kind.is_symlink() {
            bytes = bytes.saturating_add(size(&entry.path().join("workspace"))?);
        }
    }
    Ok(bytes)
}

fn modified_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn files(source: &Path, child_root: &Path, excluded: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut walk = ignore::WalkBuilder::new(source);
    walk.hidden(false)
        .parents(false)
        .require_git(false)
        .git_global(false)
        .git_exclude(false)
        .add_custom_ignore_filename(".axignore");
    let child_root = child_root.to_path_buf();
    let excluded = excluded.to_vec();
    walk.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        let name = entry.file_name().to_str().unwrap_or_default();
        !matches!(
            name,
            ".git"
                | ".ax"
                | ".workbuddy"
                | "target"
                | "node_modules"
                | "release"
                | "dist"
                | "build"
                | ".venv"
        ) && !entry.path().starts_with(&child_root)
            && !excluded.iter().any(|root| entry.path().starts_with(root))
            && !entry.path_is_symlink()
    });
    walk.build()
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_type().is_some_and(|t| t.is_file()) => {
                Some(Ok(entry.into_path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(io::Error::other(error))),
        })
        .collect()
}
fn copy_files(
    source: &Path,
    destination: &Path,
    paths: &[PathBuf],
    max_bytes: u64,
) -> io::Result<()> {
    let mut bytes = 0_u64;
    for path in paths {
        bytes = bytes.saturating_add(path.metadata()?.len());
        if bytes > max_bytes {
            return Err(io::Error::other("child snapshot disk quota exceeded"));
        }
        let destination = destination.join(path.strip_prefix(source).map_err(io::Error::other)?);
        std::fs::create_dir_all(destination.parent().unwrap())?;
        std::fs::copy(path, destination)?;
    }
    Ok(())
}

/// Only untracked files are copied for Git; tracked content comes from HEAD + patch.
// The cache lock spans clone/fetch, exact checkout and quota admission.
#[allow(clippy::too_many_lines)]
pub(super) fn provision_spec(
    source: &Path,
    destination: &Path,
    root: &Path,
    excluded: &[PathBuf],
    state: &Path,
    policy: Policy,
    spec: &runtime_core::task_queue::WorkspaceSpec,
) -> io::Result<()> {
    use runtime_core::task_queue::WorkspaceMode;
    use std::hash::{Hash, Hasher};
    match spec.mode {
        WorkspaceMode::Inherit => provision(source, destination, root, excluded, state, policy),
        WorkspaceMode::Empty => {
            std::fs::create_dir_all(destination)?;
            Ok(())
        }
        WorkspaceMode::Git => {
            // Lifecycle commands run under the configured sandbox with an
            // explicit managed-cache capability; never fall back to host mode.
            let _admission = admission(root);
            let url = spec
                .repo_url
                .as_deref()
                .ok_or_else(|| io::Error::other("missing repo_url"))?;
            if url.starts_with('-') {
                return Err(io::Error::other("invalid repo_url"));
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            url.hash(&mut hasher);
            let cache_root = root.join("git-cache");
            std::fs::create_dir_all(&cache_root)?;
            let cache = cache_root.join(format!("{:016x}.git", hasher.finish()));
            if cache
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                return Err(io::Error::other("repository cache must not be a symlink"));
            }
            let mut sandbox_policy =
                sandbox::SandboxManager::policy_for_workspace(root.to_path_buf())
                    .map_err(io::Error::other)?;
            sandbox_policy.mode = super::service_sandbox_mode();
            sandbox_policy.runtime_mounts.push(sandbox::RuntimeMount {
                path: root.to_path_buf(),
                read_only: false,
            });
            let manager =
                sandbox::SandboxManager::prepare(&sandbox_policy).map_err(io::Error::other)?;
            let run = |cwd: &Path, args: Vec<String>| -> io::Result<std::process::Output> {
                let mut command = sandbox::CommandSpec::new("git");
                command.args = vec![
                    "-c".into(),
                    "core.hooksPath=/dev/null".into(),
                    "-c".into(),
                    "core.fsmonitor=false".into(),
                    "-C".into(),
                    cwd.to_string_lossy().into_owned(),
                ];
                command.args.extend(args);
                let output = manager.output_blocking(command).map_err(io::Error::other)?;
                if !output.status.success() {
                    return Err(io::Error::other(format!(
                        "git workspace preparation: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
                Ok(output)
            };
            if cache.is_dir() {
                let configured = run(
                    &cache,
                    vec!["config".into(), "--get".into(), "remote.origin.url".into()],
                )?;
                if String::from_utf8_lossy(&configured.stdout).trim() != url {
                    return Err(io::Error::other("repository cache identity mismatch"));
                }
            } else {
                run(
                    root,
                    vec![
                        "clone".into(),
                        "--bare".into(),
                        "--filter=blob:none".into(),
                        "--no-tags".into(),
                        url.into(),
                        cache.to_string_lossy().into_owned(),
                    ],
                )?;
            }
            let revision = spec.revision.as_deref().unwrap_or("HEAD");
            if revision.starts_with('-') {
                return Err(io::Error::other("invalid revision"));
            }
            let resolve = || {
                run(
                    &cache,
                    vec![
                        "rev-parse".into(),
                        "--verify".into(),
                        format!("{revision}^{{commit}}"),
                    ],
                )
            };
            let commit = if let Ok(commit) = resolve() {
                commit
            } else {
                run(
                    &cache,
                    vec!["fetch".into(), "origin".into(), revision.into()],
                )?;
                resolve()?
            };
            let commit = String::from_utf8_lossy(&commit.stdout).trim().to_owned();
            let confined = super::service_sandbox_mode() != sandbox::SandboxMode::Off;
            if confined {
                // Standalone object store in confined children; an external
                // shared Git directory must not grant sibling write access.
                run(
                    root,
                    vec![
                        "clone".into(),
                        "--no-checkout".into(),
                        "--no-hardlinks".into(),
                        cache.to_string_lossy().into_owned(),
                        destination.to_string_lossy().into_owned(),
                    ],
                )?;
                run(
                    destination,
                    vec!["checkout".into(), "--detach".into(), commit],
                )?;
            } else {
                let mut manifest = read_manifest(state)?;
                manifest.repository = Some(cache.clone());
                write_manifest(state, &manifest)?;
                run(
                    &cache,
                    vec![
                        "worktree".into(),
                        "add".into(),
                        "--detach".into(),
                        destination.to_string_lossy().into_owned(),
                        commit,
                    ],
                )?;
            }
            invalidate_usage_cache(root);
            check_quota(root, destination, policy)
        }
    }
}

pub(super) fn provision(
    source: &Path,
    destination: &Path,
    root: &Path,
    excluded: &[PathBuf],
    state: &Path,
    policy: Policy,
) -> io::Result<()> {
    let _timer = tool::telemetry::Timer::new("child.workspace_create");
    let head = git(
        source,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("HEAD"),
        ],
    );
    let repository = head.as_ref().is_ok_and(|o| o.status.success());
    let paths = files(source, root, excluded)?;
    std::fs::create_dir_all(destination.parent().unwrap())?;
    if repository {
        // Require the project root: silently applying a subdirectory patch would be incorrect.
        let top = git(
            source,
            &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        )?;
        let repository = absolute_path(Path::new(String::from_utf8_lossy(&top.stdout).trim()))?;
        if repository != source {
            return Err(io::Error::other("child Git source must be repository root"));
        }
        // Preflight tracked checkout size before allocating the worktree.
        let tree = git(
            source,
            &[
                OsStr::new("ls-tree"),
                OsStr::new("-r"),
                OsStr::new("-l"),
                OsStr::new("HEAD"),
            ],
        )?;
        let bytes: u64 = String::from_utf8_lossy(&tree.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().nth(3)?.parse::<u64>().ok())
            .sum();
        if bytes > policy.workspace_bytes {
            return Err(io::Error::other("child Git checkout disk quota exceeded"));
        }
        let mut manifest = read_manifest(state)?;
        let confined = super::service_sandbox_mode() != sandbox::SandboxMode::Off;
        manifest.repository = if confined { None } else { Some(repository) };
        write_manifest(state, &manifest)?;
        let result = if confined {
            // Shallow clone: a disposable child needs HEAD's content, not the
            // entire object history, and the source stays read-only.
            git(
                source,
                &[
                    OsStr::new("clone"),
                    OsStr::new("--depth"),
                    OsStr::new("1"),
                    OsStr::new("--no-tags"),
                    OsStr::new("--no-hardlinks"),
                    OsStr::new("--no-checkout"),
                    source.as_os_str(),
                    destination.as_os_str(),
                ],
            )?
        } else {
            git(
                source,
                &[
                    OsStr::new("worktree"),
                    OsStr::new("add"),
                    OsStr::new("--detach"),
                    destination.as_os_str(),
                    OsStr::new(String::from_utf8_lossy(&head.as_ref().unwrap().stdout).trim()),
                ],
            )?
        };
        if !result.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&result.stderr).into_owned(),
            ));
        }
        if confined {
            let checkout = git(
                destination,
                &[
                    OsStr::new("checkout"),
                    OsStr::new("--detach"),
                    OsStr::new(String::from_utf8_lossy(&head.as_ref().unwrap().stdout).trim()),
                ],
            )?;
            if !checkout.status.success() {
                return Err(io::Error::other(
                    String::from_utf8_lossy(&checkout.stderr).into_owned(),
                ));
            }
        }
        overlay_git(source, destination, state, paths, policy)?;
    } else {
        std::fs::create_dir_all(destination)?;
        copy_files(source, destination, &paths, policy.workspace_bytes)?;
    }
    // Explicit exclusions and .axignore also apply to tracked checkout files.
    let allowed = files(destination, &destination.join(".ax"), &[])?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    prune_ignored(destination, destination, &allowed, source, excluded)?;
    check_quota(root, destination, policy)
}
fn overlay_git(
    source: &Path,
    destination: &Path,
    state: &Path,
    paths: Vec<PathBuf>,
    policy: Policy,
) -> io::Result<()> {
    let patch = git(
        source,
        &[
            OsStr::new("diff"),
            OsStr::new("--binary"),
            OsStr::new("--full-index"),
            OsStr::new("HEAD"),
            OsStr::new("--"),
        ],
    )?;
    if !patch.status.success() {
        return Err(io::Error::other("cannot capture controller dirty patch"));
    }
    if !patch.stdout.is_empty() {
        if size(destination)?.saturating_add(patch.stdout.len() as u64) > policy.workspace_bytes {
            return Err(io::Error::other("child dirty patch disk quota exceeded"));
        }
        let patch_path = state.join("input.patch");
        std::fs::write(&patch_path, patch.stdout)?;
        let result = git(
            destination,
            &[
                OsStr::new("apply"),
                OsStr::new("--binary"),
                patch_path.as_os_str(),
            ],
        );
        // Remove scratch input even when Git fails to apply it.
        std::fs::remove_file(patch_path)?;
        let result = result?;
        if !result.status.success() {
            return Err(io::Error::other(format!(
                "cannot apply controller dirty patch: {}",
                String::from_utf8_lossy(&result.stderr)
            )));
        }
    }
    let untracked = git(
        source,
        &[
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("-z"),
        ],
    )?;
    if !untracked.status.success() {
        return Err(io::Error::other("cannot enumerate untracked input"));
    }
    let untracked = untracked
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| {
            std::str::from_utf8(s)
                .map(|s| source.join(s))
                .map_err(io::Error::other)
        })
        .collect::<io::Result<std::collections::HashSet<_>>>()?;
    let paths = paths
        .into_iter()
        .filter(|p| untracked.contains(p))
        .collect::<Vec<_>>();
    copy_files(
        source,
        destination,
        &paths,
        policy.workspace_bytes.saturating_sub(size(destination)?),
    )?;
    Ok(())
}

fn prune_ignored(
    directory: &Path,
    destination: &Path,
    allowed: &std::collections::HashSet<PathBuf>,
    source: &Path,
    excluded: &[PathBuf],
) -> io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let original = source.join(path.strip_prefix(destination).map_err(io::Error::other)?);
        if entry.file_type()?.is_dir() {
            prune_ignored(&path, destination, allowed, source, excluded)?;
        } else if !allowed.contains(&path) || excluded.iter().any(|p| original.starts_with(p)) {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}
