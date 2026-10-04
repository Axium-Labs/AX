//! Non-Git content baselines preserve shell edits without inventing diff counts.
use super::{durable_write, failure, runtime_artifact};
use runtime_core::{AgentError, ChangedFile, ChildResult, DiffStat};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path};

fn inventory(cwd: &Path) -> Result<BTreeMap<String, String>, AgentError> {
    let mut files = BTreeMap::new();
    let mut walk = ignore::WalkBuilder::new(cwd);
    walk.hidden(false)
        .parents(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false);
    walk.filter_entry(|entry| {
        entry.depth() == 0
            || (!entry.path_is_symlink()
                && entry.file_name() != ".git"
                && !runtime_artifact(&entry.file_name().to_string_lossy()))
    });
    for entry in walk.build() {
        let entry = entry.map_err(failure)?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(cwd).map_err(failure)?;
        let mut input = std::fs::File::open(entry.path()).map_err(failure)?;
        let mut hash = Sha256::new();
        let mut bytes = vec![0_u8; 65536];
        loop {
            let count = input.read(&mut bytes).map_err(failure)?;
            if count == 0 {
                break;
            }
            hash.update(&bytes[..count]);
        }
        files.insert(
            relative.to_string_lossy().into_owned(),
            format!("{:x}", hash.finalize()),
        );
    }
    Ok(files)
}

pub(super) fn save(cwd: &Path, state: &Path) -> Result<(), AgentError> {
    if cwd.join(".git").exists() {
        return Ok(());
    }
    durable_write(
        &state.join("file-baseline.json"),
        &serde_json::to_vec(&inventory(cwd)?).map_err(failure)?,
    )
}

pub(super) fn apply(cwd: &Path, state: &Path, result: &mut ChildResult) -> Result<(), AgentError> {
    let current = inventory(cwd)?;
    let previous = std::fs::read(state.join("file-baseline.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, String>>(&bytes).ok());
    if let Some(previous) = previous {
        result.changed_files = current
            .iter()
            .filter(|(path, hash)| previous.get(*path) != Some(*hash))
            .map(|(path, _)| ChangedFile {
                path: path.clone(),
                change: if previous.contains_key(path) {
                    "modified"
                } else {
                    "created"
                }
                .into(),
            })
            .chain(
                previous
                    .keys()
                    .filter(|path| !current.contains_key(*path))
                    .map(|path| ChangedFile {
                        path: path.clone(),
                        change: "deleted".into(),
                    }),
            )
            .collect();
        result.diff_stat = DiffStat {
            files: result.changed_files.len(),
            ..DiffStat::default()
        };
    } else {
        // Legacy non-Git runs have no trustworthy baseline. Keep all ordinary
        // files as explicit snapshot artifacts rather than losing unknown edits.
        for path in current.keys() {
            result.artifacts.push(runtime_core::Artifact {
                path: path.clone(),
                kind: "workspace-snapshot".into(),
            });
        }
    }
    result.diagnostics.push("No Git base: binary patch and line diff counts unavailable; changed/snapshot files retained as durable artifacts".into());
    Ok(())
}
