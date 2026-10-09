//! Read-only turn baselines. Diffs compare workspace bytes before and after this run.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub type Snapshot = BTreeMap<String, Vec<u8>>;

/// Capture ordinary workspace files, including clean tracked files and non-Git projects.
/// # Errors
/// Reports snapshot failures instead of presenting them as an unchanged workspace.
pub fn snapshot(cwd: &Path) -> Result<Snapshot, String> {
    if crate::child_runtime::service_sandbox_mode() == sandbox::SandboxMode::Off {
        return snapshot_inside(cwd);
    }
    let result = (|| {
        let manager = sandbox::SandboxManager::for_workspace(cwd.to_path_buf())?;
        let mut spec = sandbox::CommandSpec::new(std::env::current_exe()?);
        spec.args.push("--ax-sandbox-snapshot".into());
        let output = manager.output_blocking(spec)?;
        if !output.status.success() {
            return Err(sandbox::SandboxViolation(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        serde_json::from_slice(&output.stdout).map_err(|e| sandbox::SandboxViolation(e.to_string()))
    })();
    result.map_err(|error| error.to_string())
}
pub(crate) fn worker_snapshot() -> anyhow::Result<()> {
    sandbox::verify_worker()?;
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
    for path in paths {
        let full = cwd.join(&path);
        if runtime_home
            .as_ref()
            .is_some_and(|home| full.starts_with(home))
        {
            continue;
        }
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_file() => {
                if !full
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(cwd)
                {
                    continue;
                }
                files.insert(path, std::fs::read(full).map_err(|e| e.to_string())?);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(files)
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

#[cfg(test)]
#[path = "../../../test/worktree_changes.rs"]
mod tests;
