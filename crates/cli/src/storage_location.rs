//! Installation-owned state and non-destructive migration from workspace state.
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, TransactionBehavior, backup::Backup};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) fn project_directory(root: &Path) -> PathBuf {
    project_directory_in(&crate::config::ax_home(), root)
}

pub(crate) fn is_project_store(data: &Path, root: &Path) -> bool {
    path_key(data) == path_key(&root.join(".ax"))
        || path_key(data) == path_key(&project_directory(root))
}

pub(crate) fn initialize(
    cwd: &Path,
    data: Option<PathBuf>,
    skills: Option<PathBuf>,
) -> Result<(PathBuf, PathBuf)> {
    migrate_home()?;
    let directories = crate::resolve_directories(cwd, data, skills);
    migrate_project(&crate::discover_project_root(cwd), &directories.0)?;
    Ok(directories)
}

fn project_directory_in(home: &Path, root: &Path) -> PathBuf {
    let key = path_key(root);
    home.join("projects")
        .join(format!("{:x}", Sha256::digest(key.as_bytes())))
}

fn path_key(path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    let key = key
        .strip_prefix(r"\\?\")
        .unwrap_or(&key)
        .replace('/', "\\")
        .to_lowercase();
    key
}

pub(crate) fn relocated_data_dir(location: &crate::session_projects::ProjectLocation) -> PathBuf {
    if path_key(&location.data_dir) == path_key(&location.root.join(".ax")) {
        project_directory(&location.root)
    } else {
        location.data_dir.clone()
    }
}

pub(crate) fn migrate_home() -> Result<()> {
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        && std::env::var_os("AX_HOME").is_none()
    {
        let legacy = PathBuf::from(home).join(".ax");
        let target = crate::config::ax_home();
        // Installers may already have placed bundled skills in the new home.
        copy_missing(&legacy, &target)?;
    }
    // Relocate known projects too, so their history survives later deletion.
    for mut project in crate::session_projects::list()? {
        let target = relocated_data_dir(&project);
        if target != project.data_dir {
            migrate_project(&project.root, &target)?;
            if let Ok(relative) = project.mcp_config.strip_prefix(&project.data_dir) {
                project.mcp_config = target.join(relative);
            }
            project.data_dir = target;
            crate::session_projects::register(project)?;
        }
    }
    Ok(())
}

pub(crate) fn migrate_project(root: &Path, target: &Path) -> Result<()> {
    if target != project_directory(root) {
        return Ok(());
    }
    copy_missing(&root.join(".ax"), target)
}

fn copy_missing(source: &Path, target: &Path) -> Result<()> {
    if !source.is_dir() || source == target || target.starts_with(source) {
        return Ok(());
    }
    fs::create_dir_all(target)?;
    let marker = target.join(format!(
        ".migrated-{:x}",
        Sha256::digest(source.to_string_lossy().as_bytes())
    ));
    if marker.is_file() {
        return Ok(());
    }
    let database = source.join("memory.sqlite3");
    if database.is_file() && !target.join("memory.sqlite3").exists() {
        // Hold the same writer lock as append_message while snapshotting SQLite
        // and its authoritative JSONL files. WAL data must not be copied raw.
        let mut connection = Connection::open(&database)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stage = target.join(format!(".migration-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&stage)?;
        let mut destination = Connection::open(stage.join("memory.sqlite3"))?;
        let reader =
            Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Backup::new(&reader, &mut destination)?.run_to_completion(
            128,
            Duration::from_millis(5),
            None,
        )?;
        drop(destination);
        copy_files(source, &stage)?;
        // Publish JSONL first and database last. Failed copies remain retryable.
        publish_events(&stage, target)?;
        copy_files(&stage, target)?;
        let published = fs::hard_link(stage.join("memory.sqlite3"), target.join("memory.sqlite3"));
        if let Err(error) = published
            && error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error).context("publish migrated AX database");
        }
        transaction.commit()?;
        fs::remove_dir_all(&stage)?;
    }
    copy_files(source, target)?;
    fs::write(marker, b"Legacy files copied; originals retained.\n")?;
    Ok(())
}

fn publish_events(stage: &Path, target: &Path) -> Result<()> {
    let events = stage.join("sessions");
    if !events.is_dir() {
        return Ok(());
    }
    let destination = target.join("sessions");
    if !destination.exists() {
        fs::rename(events, destination)?;
        return Ok(());
    }
    for entry in fs::read_dir(events)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let file = destination.join(entry.file_name());
        if file.is_file() {
            let snapshot = fs::read(entry.path())?;
            let previous = fs::read(&file)?;
            if !snapshot.starts_with(&previous) {
                bail!(
                    "conflicting JSONL at {}; original data retained",
                    file.display()
                );
            }
        }
        // Retry can replace a prefix left by an interrupted earlier copy, before
        // any destination database has been published. Both logs stay complete.
        fs::rename(entry.path(), file)?;
    }
    Ok(())
}

fn copy_files(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text.starts_with(".migration-")
            || matches!(
                name_text.as_ref(),
                "memory.sqlite3" | "memory.sqlite3-wal" | "memory.sqlite3-shm"
            )
        {
            continue;
        }
        let destination = target.join(&name);
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("cannot migrate symlink {}", entry.path().display());
        }
        if kind.is_dir() {
            copy_missing(&entry.path(), &destination)?;
        } else if !destination.exists() {
            // Create without overwriting existing installation data.
            let mut input = fs::File::open(entry.path())?;
            let temporary = target.join(format!(".migration-file-{}", uuid::Uuid::new_v4()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(mut output) => {
                    std::io::copy(&mut input, &mut output)?;
                    output.sync_all()?;
                    drop(output);
                    fs::set_permissions(&temporary, fs::metadata(entry.path())?.permissions())?;
                    match fs::hard_link(&temporary, &destination) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error.into()),
                    }
                    fs::remove_file(&temporary)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory::{MemoryStore, MessageKind, MessageRole, NewMessage};

    #[test]
    fn migration_preserves_wal_messages_and_survives_workspace_deletion() {
        let root = std::env::temp_dir().join(format!("ax-storage-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let legacy = workspace.join(".ax");
        fs::create_dir_all(&legacy).unwrap();
        let target = project_directory_in(&root.join("installation/.ax"), &workspace);
        let mut source = MemoryStore::open(legacy.join("memory.sqlite3")).unwrap();
        let session = source.create_session("preserve history").unwrap();
        source
            .append_message(
                &session.id,
                NewMessage {
                    role: MessageRole::User,
                    kind: MessageKind::Message,
                    content: "saved message".into(),
                    metadata: serde_json::Value::Null,
                },
            )
            .unwrap();
        fs::write(legacy.join("mcp.toml"), "legacy").unwrap();
        fs::create_dir_all(legacy.join("evolution")).unwrap();
        fs::write(legacy.join("evolution/experiences.jsonl"), "experience").unwrap();
        source
            .remember_scoped(&memory::MemoryRecord {
                scope: memory::MemoryScope::Project,
                owner: "project-owner".into(),
                key: "build".into(),
                value: "cargo test".into(),
                source: "user".into(),
                updated_at: 0,
                always_include: false,
                ..Default::default()
            })
            .unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("mcp.toml"), "existing").unwrap();
        fs::create_dir_all(target.join("sessions")).unwrap();
        fs::write(
            target
                .join("sessions")
                .join(format!("{}.jsonl", session.id)),
            b"{",
        )
        .unwrap();
        copy_missing(&legacy, &target).unwrap();
        assert!(legacy.join("memory.sqlite3").exists());
        copy_missing(&legacy, &target).unwrap();
        drop(source);
        fs::remove_dir_all(&workspace).unwrap();
        let migrated = MemoryStore::open(target.join("memory.sqlite3")).unwrap();
        assert!(migrated.session(&session.id).unwrap().is_some());
        assert_eq!(
            migrated.load_messages(&session.id, None, 10).unwrap()[0].content,
            "saved message"
        );
        assert_eq!(
            fs::read_to_string(target.join("mcp.toml")).unwrap(),
            "existing"
        );
        assert_eq!(
            migrated
                .scoped_memories(memory::MemoryScope::Project, "project-owner")
                .unwrap()[0]
                .value,
            "cargo test"
        );
        assert_eq!(
            fs::read_to_string(target.join("evolution/experiences.jsonl")).unwrap(),
            "experience"
        );
        drop(migrated);
        fs::remove_dir_all(root).unwrap();
    }
}
