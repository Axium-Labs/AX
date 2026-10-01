//! Shared, metadata-only scope resolution. No capability execution belongs here.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}
impl Scope {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScopePolicy {
    #[serde(default)]
    pub disabled_global: BTreeSet<String>,
    #[serde(default)]
    pub overrides: BTreeMap<String, bool>,
}

#[derive(Clone, Debug)]
pub struct ScopedEntry<T> {
    pub name: String,
    pub scope: Scope,
    pub enabled: bool,
    pub disabled_here: bool,
    pub value: T,
}
impl<T> ScopedEntry<T> {
    #[must_use]
    pub const fn status(&self) -> &'static str {
        if self.disabled_here {
            "disabled here"
        } else if self.enabled {
            "enabled"
        } else {
            "disabled"
        }
    }
}

/// Project definitions replace whole global definitions before project masks apply.
#[derive(Clone, Debug)]
pub struct ScopedRegistry<T> {
    entries: BTreeMap<String, ScopedEntry<T>>,
}
impl<T> ScopedRegistry<T> {
    #[must_use]
    pub fn build(
        global: impl IntoIterator<Item = (String, T, bool)>,
        project: impl IntoIterator<Item = (String, T, bool)>,
        global_policy: &ScopePolicy,
        project_policy: &ScopePolicy,
    ) -> Self {
        let mut entries = BTreeMap::new();
        for (scope, values, policy) in [
            (
                Scope::Global,
                global.into_iter().collect::<Vec<_>>(),
                global_policy,
            ),
            (
                Scope::Project,
                project.into_iter().collect(),
                project_policy,
            ),
        ] {
            for (name, value, enabled) in values {
                let enabled = policy.overrides.get(&name).copied().unwrap_or(enabled);
                entries.insert(
                    name.clone(),
                    ScopedEntry {
                        name,
                        scope,
                        enabled,
                        disabled_here: false,
                        value,
                    },
                );
            }
        }
        for entry in entries.values_mut() {
            if let Some(enabled) = project_policy.overrides.get(&entry.name) {
                entry.enabled = *enabled;
            }
            if entry.scope == Scope::Global && project_policy.disabled_global.contains(&entry.name)
            {
                entry.enabled = false;
                entry.disabled_here = true;
            }
        }
        Self { entries }
    }
    pub fn entries(&self) -> impl Iterator<Item = &ScopedEntry<T>> {
        self.entries.values()
    }
    pub fn effective(&self) -> impl Iterator<Item = &ScopedEntry<T>> {
        self.entries().filter(|entry| entry.enabled)
    }
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ScopedEntry<T>> {
        self.entries.get(name)
    }
}

/// Parse the same policy format for every capability, retaining unrelated config.
/// # Errors
/// Returns file IO or TOML parse errors.
pub fn read_document(path: &Path) -> Result<toml::Value> {
    match fs::read_to_string(path) {
        Ok(contents) => toml::from_str(&contents)
            .with_context(|| format!("Invalid configuration {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(toml::Value::Table(toml::map::Map::new()))
        }
        Err(error) => Err(error).with_context(|| format!("Cannot read {}", path.display())),
    }
}
/// # Errors
/// Returns invalid configuration or policy field errors.
pub fn policy(path: &Path, kind: &str) -> Result<ScopePolicy> {
    let document = read_document(path)?;
    let mut result: ScopePolicy = document
        .get(kind)
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()))
        .try_into()?;
    if let Some(disabled) = document.get("disabled_global") {
        result
            .disabled_global
            .extend(disabled.clone().try_into::<BTreeSet<String>>()?);
    }
    Ok(result)
}
/// # Errors
/// Returns serialization or atomic file publication errors.
pub fn write_document(path: &Path, document: &toml::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temporary, toml::to_string_pretty(document)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}
/// # Errors
/// Returns configuration read, serialization or write errors.
pub fn save_policy(path: &Path, kind: &str, policy: &ScopePolicy) -> Result<()> {
    let mut document = read_document(path)?;
    document
        .as_table_mut()
        .context("Configuration must be a table")?
        .insert(kind.into(), toml::Value::try_from(policy)?);
    write_document(path, &document)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_replaces_before_global_mask_and_disabled_never_effective() {
        let global = [
            ("same".into(), 1, true),
            ("browser".into(), 2, true),
            ("off".into(), 3, false),
        ];
        let project = [("same".into(), 4, true)];
        let policy = ScopePolicy {
            disabled_global: BTreeSet::from(["same".into(), "browser".into()]),
            ..ScopePolicy::default()
        };
        let registry = ScopedRegistry::build(global, project, &ScopePolicy::default(), &policy);
        assert_eq!(
            registry.effective().map(|e| e.value).collect::<Vec<_>>(),
            vec![4]
        );
        assert_eq!(registry.get("same").unwrap().scope, Scope::Project);
        assert_eq!(registry.get("browser").unwrap().status(), "disabled here");
    }
    #[test]
    fn override_does_not_resurrect_a_masked_global() {
        let policy = ScopePolicy {
            disabled_global: BTreeSet::from(["x".into()]),
            overrides: BTreeMap::from([("x".into(), true)]),
        };
        let registry = ScopedRegistry::build(
            [("x".into(), (), false)],
            [],
            &ScopePolicy::default(),
            &policy,
        );
        assert_eq!(registry.effective().count(), 0);
    }
}
