//! Paths of projects whose sessions AX has opened on this machine.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectLocation {
    pub id: String,
    pub root: PathBuf,
    pub data_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub mcp_config: PathBuf,
}

fn path() -> PathBuf {
    crate::config::ax_home().join("session-projects.json")
}

pub fn list() -> Result<Vec<ProjectLocation>> {
    let path = path();
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

pub fn register(project: ProjectLocation) -> Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    let mut projects = list()?;
    projects.retain(|existing| existing.id != project.id);
    projects.insert(0, project);
    let stage = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&stage, serde_json::to_vec_pretty(&projects)?)?;
    fs::rename(stage, path)?;
    Ok(())
}
