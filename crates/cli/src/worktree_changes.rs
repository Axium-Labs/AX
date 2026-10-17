//! Read-only turn baselines. Diffs compare workspace bytes before and after this run.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

pub type Snapshot = BTreeMap<String, Vec<u8>>;

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);

/// Capture ordinary workspace files, including clean tracked files and non-Git projects.
/// # Errors
/// Reports snapshot failures instead of presenting them as an unchanged workspace.
pub async fn snapshot(cwd: &Path) -> Result<Snapshot, String> {
    // A cancellable child bounds even slow filesystem/Git operations. Never run
    // a synchronous workspace scan on the ACP executor or keep an orphan scan.
    let result = async {
        let manager = sandbox::SandboxManager::for_workspace(cwd.to_path_buf())?;
        let mut spec = sandbox::CommandSpec::new(std::env::current_exe()?);
        spec.cwd = Some(cwd.to_path_buf());
        spec.args
            .push(if manager.policy().mode == sandbox::SandboxMode::Off {
                "--ax-worktree-snapshot".into()
            } else {
                "--ax-sandbox-snapshot".into()
            });
        let output = manager.spawn(spec)?.wait_with_output().await?;
        if !output.status.success() {
            return Err(sandbox::SandboxViolation(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|e| sandbox::SandboxViolation(e.to_string()))
    };
    tokio::time::timeout(SNAPSHOT_TIMEOUT, result)
        .await
        .map_err(|_| {
            "workspace change preview unavailable: snapshot exceeded 2 seconds".to_owned()
        })?
        .map_err(|error: sandbox::SandboxViolation| error.to_string())
}
pub(crate) fn worker_snapshot() -> anyhow::Result<()> {
    sandbox::verify_worker()?;
    host_snapshot()
}

pub(crate) fn host_snapshot() -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string(
            &snapshot_inside(&std::env::current_dir()?).map_err(anyhow::Error::msg)?
        )?
    );
    Ok(())
}

fn ordinary(path: &Path) -> bool {
    !path.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some(".git" | ".ax" | "node_modules" | "target" | "__pycache__")
        )
    })
}
fn snapshot_inside(cwd: &Path) -> Result<Snapshot, String> {
    let started = Instant::now();
    let canonical = cwd.canonicalize().map_err(|e| e.to_string())?;
    let cwd = canonical.as_path();
    let runtime_home = crate::config::ax_home().canonicalize().ok();
    let mut paths = BTreeSet::new();
    // NUL framing handles spaces, Unicode and tabs; no HEAD is needed in new repositories.
    let git = std::process::Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success());
    if let Some(git) = git {
        for raw in git
            .stdout
            .split(|byte| *byte == 0)
            .filter(|p| !p.is_empty())
        {
            check_limits(started, paths.len(), 0)?;
            let path = String::from_utf8(raw.to_vec()).map_err(|e| e.to_string())?;
            if ordinary(Path::new(&path)) {
                paths.insert(path);
            }
        }
    } else {
        let mut walk = ignore::WalkBuilder::new(cwd);
        walk.hidden(false).parents(false).require_git(false);
        let home = runtime_home.clone();
        walk.filter_entry(move |entry| {
            entry.depth() == 0
                || (!entry.path_is_symlink()
                    && ordinary(entry.path())
                    && !home
                        .as_ref()
                        .is_some_and(|home| entry.path().starts_with(home)))
        });
        for entry in walk.build() {
            check_limits(started, paths.len(), 0)?;
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().is_some_and(|kind| kind.is_file()) {
                paths.insert(
                    entry
                        .path()
                        .strip_prefix(cwd)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    let mut bytes = 0;
    for path in paths {
        check_limits(started, files.len(), bytes)?;
        let full = cwd.join(&path);
        if runtime_home
            .as_ref()
            .is_some_and(|home| full.starts_with(home))
        {
            continue;
        }
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() => {
                if meta.len() > MAX_FILE_BYTES {
                    return Err(format!(
                        "workspace change preview unavailable: {path} exceeds the 2 MiB file limit"
                    ));
                }
                if bytes.saturating_add(meta.len()) > MAX_SNAPSHOT_BYTES {
                    return Err(
                        "workspace change preview unavailable: snapshot exceeds 16 MiB".into(),
                    );
                }
                if !full
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(cwd)
                {
                    continue;
                }
                let content = read_bounded(&full)?;
                bytes += content.len() as u64;
                check_limits(started, files.len(), bytes)?;
                files.insert(path, content);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(files)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    // The file can grow after metadata was read. Bound the read too.
    let mut content = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|e| e.to_string())?;
    if content.len() as u64 > MAX_FILE_BYTES {
        return Err("workspace change preview unavailable: file grew beyond 2 MiB".into());
    }
    Ok(content)
}

fn check_limits(started: Instant, files: usize, bytes: u64) -> Result<(), String> {
    if started.elapsed() >= Duration::from_secs(1)
        || files >= MAX_FILES
        || bytes > MAX_SNAPSHOT_BYTES
    {
        return Err("workspace change preview unavailable: snapshot reached its time, file-count or memory limit".into());
    }
    Ok(())
}

struct DiffTemp(std::path::PathBuf);
impl Drop for DiffTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn text_diff(path: &str, before: Option<&Vec<u8>>, after: Option<&Vec<u8>>) -> Option<String> {
    let old = before.map_or(&[][..], Vec::as_slice);
    let new = after.map_or(&[][..], Vec::as_slice);
    let (Ok(old_text), Ok(new_text)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return None;
    };
    if old.contains(&0) || new.contains(&0) {
        return None;
    }
    let mut body = None;
    let temp =
        DiffTemp(std::env::temp_dir().join(format!("ax-turn-diff-{}", uuid::Uuid::new_v4())));
    if std::fs::create_dir(&temp.0).is_ok()
        && std::fs::write(temp.0.join("before"), old).is_ok()
        && std::fs::write(temp.0.join("after"), new).is_ok()
        && let Ok(output) = std::process::Command::new("git")
            .args([
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--no-renames",
                "--unified=3",
                "--",
                "before",
                "after",
            ])
            .current_dir(&temp.0)
            .output()
        && matches!(output.status.code(), Some(0 | 1))
    {
        let raw = String::from_utf8_lossy(&output.stdout);
        if let Some(start) = raw.find("@@ ") {
            body = Some(raw[start..].to_owned());
        }
    }
    let body = body.unwrap_or_else(|| {
        // Without Git a complete replacement still provides a valid, inspectable diff.
        let mut body = format!(
            "@@ -{},{} +{},{} @@\n",
            usize::from(!old.is_empty()),
            old_text.lines().count(),
            usize::from(!new.is_empty()),
            new_text.lines().count()
        );
        for (prefix, text) in [('-', old_text), ('+', new_text)] {
            for line in text.lines() {
                body.push(prefix);
                body.push_str(line);
                body.push('\n');
            }
            if !text.is_empty() && !text.ends_with('\n') {
                body.push_str("\\ No newline at end of file\n");
            }
        }
        body
    });
    Some(format!(
        "--- {}\n+++ {}\n{body}",
        before.map_or_else(|| "/dev/null".into(), |_| format!("a/{path}")),
        after.map_or_else(|| "/dev/null".into(), |_| format!("b/{path}"))
    ))
}

pub fn changed(before: &Snapshot, after: &Snapshot) -> Vec<Value> {
    let paths: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    paths.into_iter().filter_map(|path| {
        let old=before.get(path);let new=after.get(path);
        if old==new { return None; }
        let diff=text_diff(path,old,new);
        let additions=diff.as_deref().map_or(0,|diff|diff.lines().skip(2).filter(|line|line.starts_with('+')).count());
        let deletions=diff.as_deref().map_or(0,|diff|diff.lines().skip(2).filter(|line|line.starts_with('-')).count());
        Some(json!({"path":path,"additions":additions,"deletions":deletions,"diff":diff,"binary":diff.is_none(),"change":if old.is_none(){"created"}else if new.is_none(){"deleted"}else{"modified"}}))
    }).collect()
}
