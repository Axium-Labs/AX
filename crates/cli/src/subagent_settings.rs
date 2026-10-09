//! AX-owned global/project delegation settings, shared by CLI and runtime setup.
use anyhow::{Result, ensure};
use runtime_core::SubagentConfig;
use std::path::Path;

fn validate(config: SubagentConfig) -> Result<SubagentConfig> {
    ensure!(
        (1..=64).contains(&config.max_concurrent),
        "max_concurrent must be 1..=64"
    );
    // Project overrides are TOML integers; use the same portable domain globally.
    ensure!(
        i64::try_from(config.max_depth).is_ok(),
        "max_depth is too large"
    );
    Ok(config)
}

pub(crate) fn effective(global: SubagentConfig, root: &Path) -> Result<SubagentConfig> {
    let document = scoped::read_document(&root.join(".ax/config.toml"))?;
    let mut merged = toml::Value::try_from(global)?;
    if let Some(project) = document.get("subagent") {
        let project = project
            .as_table()
            .ok_or_else(|| anyhow::anyhow!("subagent must be a table"))?;
        merged
            .as_table_mut()
            .expect("settings table")
            .extend(project.clone());
    }
    validate(merged.try_into()?)
}

fn update_project(
    root: &Path,
    max_concurrent: Option<usize>,
    max_depth: Option<usize>,
    reset: bool,
) -> Result<()> {
    let path = root.join(".ax/config.toml");
    let mut document = scoped::read_document(&path)?;
    let table = document.as_table_mut().expect("configuration table");
    if reset {
        if let Some(settings) = table.get_mut("subagent") {
            let settings = settings
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("subagent must be a table"))?;
            settings.remove("max_concurrent");
            settings.remove("max_depth");
            if settings.is_empty() {
                table.remove("subagent");
            }
        }
    } else {
        let settings = table
            .entry("subagent")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let settings = settings
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("subagent must be a table"))?;
        for (key, value) in [("max_concurrent", max_concurrent), ("max_depth", max_depth)] {
            if let Some(value) = value {
                settings.insert(key.into(), toml::Value::Integer(i64::try_from(value)?));
            }
        }
    }
    scoped::write_document(&path, &document)
}

pub(crate) fn manage(
    root: &Path,
    scope: &str,
    max_concurrent: Option<usize>,
    max_depth: Option<usize>,
    reset: bool,
) -> Result<SubagentConfig> {
    ensure!(
        ["global", "project"].contains(&scope),
        "invalid settings scope"
    );
    let mut global = crate::config::AxConfig::load()?;
    let mut next = if scope == "project" {
        effective(global.subagent, root)?
    } else {
        global.subagent
    };
    if let Some(value) = max_concurrent {
        next.max_concurrent = value;
    }
    if let Some(value) = max_depth {
        next.max_depth = value;
    }
    validate(next)?;
    if max_concurrent.is_some() || max_depth.is_some() || reset {
        if scope == "project" {
            update_project(root, max_concurrent, max_depth, reset)?;
        } else {
            global.subagent = if reset {
                SubagentConfig::default()
            } else {
                next
            };
            global.save()?;
        }
    }
    if scope == "project" {
        effective(global.subagent, root)
    } else {
        validate(global.subagent)
    }
}

#[cfg(test)]
#[path = "../../../test/subagent_settings.rs"]
mod tests;
