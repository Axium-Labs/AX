//! Metadata-only discovery and atomic installation; never evaluates JavaScript.
use crate::capabilities::Capability;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
pub(super) struct Manifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    pub entry: PathBuf,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default, rename = "userConfig")]
    pub user_config: BTreeMap<String, Value>,
}
const fn enabled() -> bool {
    true
}

pub(super) fn read(root: &Path) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_slice(&fs::read(root.join("mod.json"))?)?;
    crate::capabilities::validate_name(&manifest.name)?;
    if manifest.entry.is_absolute()
        || manifest
            .entry
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("Mod entry must be relative to its package");
    }
    let entry = fs::canonicalize(root.join(&manifest.entry)).context("Missing Mod entry module")?;
    if !entry.starts_with(fs::canonicalize(root)?) || !entry.is_file() {
        bail!("Mod entry escapes its package");
    }
    if !manifest
        .entry
        .extension()
        .is_some_and(|e| e == "mjs" || e == "js")
    {
        bail!("Mod entry must be JavaScript (.mjs or .js); compile TypeScript first");
    }
    for value in manifest.user_config.values() {
        if !(value.is_string()
            || value.is_boolean()
            || value.is_number()
            || value
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_string)))
        {
            bail!("Mod userConfig values must be strings, numbers, booleans or string lists");
        }
    }
    Ok(manifest)
}

pub(crate) fn definitions(root: &Path) -> Result<BTreeMap<String, (Capability, bool)>> {
    let directory = root.join("mods");
    if !directory.exists() {
        return Ok(BTreeMap::new());
    }
    let mut entries = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if !path.is_dir() || !path.join("mod.json").is_file() {
            continue;
        }
        let manifest = read(&path).with_context(|| format!("Invalid Mod {}", path.display()))?;
        let name = manifest.name;
        if entries.insert(name.clone(), (Capability {
            description: manifest.description,
            source: path.clone(),
            data: json!({"entry":path.join(manifest.entry),"version":manifest.version,"userConfig":manifest.user_config}),
        }, manifest.enabled)).is_some() { bail!("Duplicate Mod: {name}"); }
    }
    Ok(entries)
}

fn copy_directory(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir(target)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        let kind = item.file_type()?;
        if kind.is_symlink() {
            bail!("Mod packages cannot contain symlinks");
        }
        if kind.is_dir() {
            copy_directory(&item.path(), &target.join(item.file_name()))?;
        } else if kind.is_file() {
            fs::copy(item.path(), target.join(item.file_name()))?;
        } else {
            bail!("Mod packages may contain only regular files and directories");
        }
    }
    Ok(())
}
pub(crate) fn install(source: &Path, root: &Path, name: &str) -> Result<()> {
    if read(source)?.name != name {
        bail!("Mod source name does not match {name}");
    }
    let directory = root.join("mods");
    fs::create_dir_all(&directory)?;
    let target = directory.join(name);
    if target.exists() {
        bail!("Mod already exists: {name}");
    }
    let staging = directory.join(format!(".install-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        copy_directory(source, &staging)?;
        read(&staging)?;
        fs::rename(&staging, &target)?;
        Ok(())
    })();
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    result
}
pub(crate) fn remove(root: &Path, entry: &Capability) -> Result<()> {
    let directory = fs::canonicalize(root.join("mods"))?;
    let path = fs::canonicalize(&entry.source)?;
    if path.parent() != Some(directory.as_path()) {
        bail!("Cannot remove Mod outside the selected scope");
    }
    fs::remove_dir_all(path)?;
    Ok(())
}
