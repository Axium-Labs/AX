//! Structured tool permissions shared by UI and runtime.
use crate::SafetyLevel;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    Shell,
    FilesystemRead,
    FilesystemWrite,
    Network,
    Mcp,
    Process,
}
impl Capability {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::FilesystemRead => "filesystem-read",
            Self::FilesystemWrite => "filesystem-write",
            Self::Network => "network",
            Self::Mcp => "mcp",
            Self::Process => "process",
        }
    }
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        [
            Self::Shell,
            Self::FilesystemRead,
            Self::FilesystemWrite,
            Self::Network,
            Self::Mcp,
            Self::Process,
        ]
        .into_iter()
        .find(|capability| capability.key() == key)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolPermission {
    pub capability: Capability,
    pub safety: SafetyLevel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}
impl std::fmt::Display for PermissionDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Allow => "Allow",
            Self::Ask => "Ask",
            Self::Deny => "Deny",
        })
    }
}
/// Clones share the same policy map; no runtime/UI snapshots.
#[derive(Clone, Debug)]
pub struct PermissionStore(Arc<RwLock<PermissionState>>);
#[derive(Debug)]
struct PermissionState {
    policies: BTreeMap<Capability, PermissionDecision>,
    session_grants: BTreeSet<Capability>,
}
impl Default for PermissionStore {
    fn default() -> Self {
        Self(Arc::new(RwLock::new(PermissionState {
            policies: BTreeMap::from([
                (Capability::FilesystemRead, PermissionDecision::Allow),
                (Capability::Network, PermissionDecision::Allow),
            ]),
            session_grants: BTreeSet::new(),
        })))
    }
}
impl PermissionStore {
    #[must_use]
    pub fn decision(&self, capability: Capability) -> PermissionDecision {
        self.0.read().map_or(PermissionDecision::Deny, |state| {
            let decision = state
                .policies
                .get(&capability)
                .copied()
                .unwrap_or(PermissionDecision::Ask);
            if decision == PermissionDecision::Ask && state.session_grants.contains(&capability) {
                PermissionDecision::Allow
            } else {
                decision
            }
        })
    }
    #[must_use]
    pub fn get(&self, key: &str) -> PermissionDecision {
        Capability::from_key(key).map_or(PermissionDecision::Deny, |capability| {
            self.decision(capability)
        })
    }
    pub fn set(&self, key: impl AsRef<str>, decision: PermissionDecision) {
        if let Some(capability) = Capability::from_key(key.as_ref()) {
            self.set_capability(capability, decision);
        }
    }
    pub fn set_capability(&self, capability: Capability, decision: PermissionDecision) {
        if let Ok(mut policies) = self.0.write() {
            policies.policies.insert(capability, decision);
            policies.session_grants.remove(&capability);
        }
    }
    pub fn allow_session(&self, capability: Capability) {
        if let Ok(mut state) = self.0.write() {
            state.session_grants.insert(capability);
        }
    }
    pub fn reset_session(&self) {
        if let Ok(mut state) = self.0.write() {
            state.session_grants.clear();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_changes_are_visible_to_existing_runtime_handles() {
        let ui = PermissionStore::default();
        let runtime = ui.clone();
        ui.set_capability(Capability::Mcp, PermissionDecision::Deny);
        assert_eq!(runtime.decision(Capability::Mcp), PermissionDecision::Deny);
        runtime.set_capability(Capability::Mcp, PermissionDecision::Allow);
        assert_eq!(ui.get("mcp"), PermissionDecision::Allow);
        assert_eq!(ui.get("process"), PermissionDecision::Ask);
    }
    #[test]
    fn session_grants_expire_and_cannot_override_a_deny() {
        let store = PermissionStore::default();
        store.allow_session(Capability::Shell);
        assert_eq!(store.get("shell"), PermissionDecision::Allow);
        store.reset_session();
        assert_eq!(store.get("shell"), PermissionDecision::Ask);
        store.set("shell", PermissionDecision::Deny);
        store.allow_session(Capability::Shell);
        assert_eq!(store.get("shell"), PermissionDecision::Deny);
    }
}
