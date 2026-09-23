//! Portable project identity stored in `.ax/project.json`.
use std::{fs, io::Write, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct ProjectMetadata {
    id: String,
}

pub(crate) fn load_existing(root: &Path) -> Result<Option<String>> {
    let directory = root.join(".ax");
    let path = directory.join("project.json");
    match fs::read_to_string(&path) {
        Ok(value) => {
            let metadata: ProjectMetadata =
                serde_json::from_str(&value).context("Invalid project metadata")?;
            Ok(Some(uuid::Uuid::parse_str(&metadata.id)?.to_string()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::read_to_string(directory.join("project-id")) {
                Ok(value) => Ok(Some(uuid::Uuid::parse_str(value.trim())?.to_string())),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error).context("Cannot read legacy project identity"),
            }
        }
        Err(error) => Err(error).context("Cannot read project identity"),
    }
}

pub(crate) fn load_or_create(root: &Path) -> Result<String> {
    let directory = root.join(".ax");
    fs::create_dir_all(&directory)?;
    let path = directory.join("project.json");
    match fs::read_to_string(&path) {
        Ok(value) => {
            let metadata: ProjectMetadata =
                serde_json::from_str(&value).context("Invalid project metadata")?;
            return Ok(uuid::Uuid::parse_str(&metadata.id)?.to_string());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Cannot read project identity"),
    }
    let legacy = directory.join("project-id");
    let id = match fs::read_to_string(&legacy) {
        Ok(value) => uuid::Uuid::parse_str(value.trim())?.to_string(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            uuid::Uuid::new_v4().to_string()
        }
        Err(error) => return Err(error).context("Cannot read legacy project identity"),
    };
    let temporary = directory.join(format!("project.{}.tmp", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(serde_json::to_string_pretty(&ProjectMetadata { id: id.clone() })?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    // Linking publishes a complete file without replacing another process's ID.
    let published = fs::hard_link(&temporary, &path);
    fs::remove_file(&temporary)?;
    match published {
        Ok(()) => {
            if legacy.exists() {
                fs::remove_file(legacy)?;
            }
            Ok(id)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata: ProjectMetadata = serde_json::from_str(&fs::read_to_string(path)?)?;
            Ok(uuid::Uuid::parse_str(&metadata.id)?.to_string())
        }
        Err(error) => Err(error).context("Cannot publish project identity"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_moves_and_different_projects_get_different_ids() {
        let root = std::env::temp_dir().join(format!("ax-identity-{}", uuid::Uuid::new_v4()));
        let first = root.join("first");
        let moved = root.join("moved");
        let id = load_or_create(&first).unwrap();
        let database = first.join(".ax/memory.sqlite3");
        let store = memory::MemoryStore::open(database).unwrap();
        store
            .remember_scoped(&memory::MemoryRecord {
                scope: memory::MemoryScope::Project,
                owner: id.clone(),
                key: "build".into(),
                value: "cargo test".into(),
                source: "test".into(),
                updated_at: 0,
                always_include: false,
            })
            .unwrap();
        drop(store);
        fs::rename(&first, &moved).unwrap();
        assert_eq!(load_or_create(&moved).unwrap(), id);
        let store = memory::MemoryStore::open(moved.join(".ax/memory.sqlite3")).unwrap();
        assert_eq!(
            store
                .scoped_memories(memory::MemoryScope::Project, &id)
                .unwrap()[0]
                .value,
            "cargo test"
        );
        drop(store);
        assert_ne!(load_or_create(&root.join("other")).unwrap(), id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_project_id_migrates_to_json_without_changing_identity() {
        let root =
            std::env::temp_dir().join(format!("ax-project-migrate-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join(".ax")).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        fs::write(root.join(".ax/project-id"), &id).unwrap();
        assert_eq!(load_or_create(&root).unwrap(), id);
        let metadata: ProjectMetadata =
            serde_json::from_str(&fs::read_to_string(root.join(".ax/project.json")).unwrap())
                .unwrap();
        assert_eq!(metadata.id, id);
        assert!(!root.join(".ax/project-id").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
