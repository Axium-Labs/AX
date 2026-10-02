//! `ax export` / `ax import`: versioned `.axpack` archives.
//!
//! Both commands open the project and global stores directly and delegate the
//! package format to `memory::backup`; no session or agent state is touched.

use std::{fs, path::Path};

use anyhow::Result;
use memory::MemoryStore;

use crate::{
    bootstrap::{database_path, discover_project_root},
    config, project_identity, storage_location,
};

pub(crate) fn run_export(
    path: &Path,
    data_dir: &Path,
    cwd: &Path,
    memory: bool,
    sessions: bool,
) -> Result<()> {
    let project_root = discover_project_root(cwd);
    let project_path = database_path(data_dir);
    let project_id = if project_path.exists() {
        project_identity::load_or_create(&project_root)?
    } else {
        project_identity::load_existing(&project_root)?
            .unwrap_or_else(|| uuid::Uuid::nil().to_string())
    };
    let global_path = config::ax_home().join("memory.sqlite3");
    let project = if project_path.exists() {
        MemoryStore::open(&project_path)?
    } else {
        MemoryStore::open_in_memory()?
    };
    if project_path.exists() {
        project.migrate_project_owner(
            &project_id,
            &project_root.to_string_lossy(),
            storage_location::is_project_store(data_dir, &project_root),
        )?;
    }
    let global = if global_path.exists() {
        MemoryStore::open(&global_path)?
    } else {
        MemoryStore::open_in_memory()?
    };
    let selection = if memory || sessions {
        memory::backup::ExportSelection { memory, sessions }
    } else {
        memory::backup::ExportSelection::all()
    };
    let report = memory::backup::ExportService {
        project: &project,
        global: &global,
        project_id: &project_id,
    }
    .export(path, selection)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
pub(crate) fn run_import(path: &Path, data_dir: &Path, cwd: &Path, dry_run: bool) -> Result<()> {
    let project_root = discover_project_root(cwd);
    let project_path = database_path(data_dir);
    let global_path = config::ax_home().join("memory.sqlite3");
    let report = if dry_run {
        let project_id = project_identity::load_existing(&project_root)?;
        memory::backup::ImportService::dry_run(
            path,
            &project_path,
            &global_path,
            project_id.as_deref(),
        )?
    } else {
        // Reject invalid packages before creating an identity or opening stores.
        let existing_project_id = project_identity::load_existing(&project_root)?;
        memory::backup::ImportService::dry_run(
            path,
            &project_path,
            &global_path,
            existing_project_id.as_deref(),
        )?;
        let project_id = project_identity::load_or_create(&project_root)?;
        if let Some(parent) = project_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut project = MemoryStore::open(&project_path)?;
        memory::backup::ImportService {
            project: &mut project,
            global_path: &global_path,
            target_project_id: &project_id,
        }
        .import(path)?
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
#[cfg(test)]
mod backup_cli_tests {
    use clap::Parser;

    use crate::args::{Cli, Command};
    #[test]
    fn parses_export_selection_and_import_dry_run() {
        let export = Cli::try_parse_from(["ax", "export", "backup.axpack", "--memory"]).unwrap();
        assert!(matches!(
            export.command,
            Some(Command::Export {
                memory: true,
                sessions: false,
                ..
            })
        ));
        let import = Cli::try_parse_from(["ax", "import", "backup.axpack", "--dry-run"]).unwrap();
        assert!(matches!(
            import.command,
            Some(Command::Import { dry_run: true, .. })
        ));
    }
}
