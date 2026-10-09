//! Lazy Node Mod host. Scope metadata and JavaScript execution remain separate.
mod catalog;
mod host;
mod tools;
pub(crate) use catalog::{definitions, install, remove};
pub(crate) use host::ModHost;
pub(crate) use host::{close_all, close_session};

pub(crate) fn revision(state: &crate::repl::ReplState) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    state
        .capability_registries
        .borrow_mut()
        .remove(&crate::capabilities::Kind::Mods);
    let registry = state.capability_registry(crate::capabilities::Kind::Mods)?;
    let mut digest = Sha256::new();
    digest.update(state.project_root.to_string_lossy().as_bytes());
    digest.update(state.capability_home.to_string_lossy().as_bytes());
    for entry in registry.entries() {
        digest.update(entry.name.as_bytes());
        digest.update([u8::from(entry.enabled)]);
        digest.update(serde_json::to_vec(&entry.value.data)?);
        let path: std::path::PathBuf = serde_json::from_value(entry.value.data["entry"].clone())?;
        digest.update(std::fs::read(path)?);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
#[path = "../../../../test/mods/runtime.rs"]
mod tests;
