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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Runtime rule types. Rules are data, never prompt instructions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleMatcher {
    Command {
        pattern: String,
    },
    Prefix {
        value: String,
    },
    Path {
        pattern: String,
    },
    Domain {
        pattern: String,
    },
    ToolParameter {
        tool: String,
        pointer: String,
        pattern: String,
    },
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PermissionRule {
    pub decision: PermissionDecision,
    pub matcher: RuleMatcher,
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PermissionProfile {
    pub rules: Vec<PermissionRule>,
    pub boundary: SandboxBoundary,
}
/// Ceiling applied in addition to rule decisions and existing capability approvals.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SandboxBoundary {
    pub read_only: bool,
    pub deny_network: bool,
}
fn glob(pattern: &str, value: &str) -> bool {
    let p: Vec<_> = pattern.chars().collect();
    let v: Vec<_> = value.chars().collect();
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < v.len() {
        if i < p.len() && p[i] == v[j] {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            retry += 1;
            j = retry;
            i = s + 1;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}
/// Path-aware glob for path rules: every segment is matched independently, so a
/// trailing `*` in `<dir>/*` covers `<dir>/file` but never the sibling
/// `<dir>-other/file`. Cross-segment matching stays with the explicit `/**`
/// form, which is deliberately not an `Allow` subtree wildcard.
fn path_glob(pattern: &str, value: &str) -> bool {
    let pattern = pattern.split('/').collect::<Vec<_>>();
    let value = value.split('/').collect::<Vec<_>>();
    pattern.len() == value.len()
        && pattern
            .iter()
            .zip(&value)
            .all(|(pattern, value)| glob(pattern, value))
}
/// Outcome of evaluating ordered permission profiles against one tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileDecision {
    /// At least one profile denies the call.
    Deny,
    /// No profile denies, but at least one requires explicit approval.
    Ask,
    /// The leading profile explicitly allows; no approval path is needed.
    Allowed,
    /// No profile expressed an opinion; the safety policy decides.
    Unspecified,
}
/// Resolves per-profile decisions into one outcome for a call.
///
/// Profiles are ordered parent-first. `Deny` wins outright and an `Ask`
/// anywhere still asks. Only the *leading* profile can short-circuit the
/// approval path with an explicit `Allow`, so a later (child-supplied) profile
/// can narrow a parent decision but never widen it.
#[must_use]
pub fn resolve_profiles(decisions: &[Option<PermissionDecision>]) -> ProfileDecision {
    if decisions.contains(&Some(PermissionDecision::Deny)) {
        return ProfileDecision::Deny;
    }
    if decisions.contains(&Some(PermissionDecision::Ask)) {
        return ProfileDecision::Ask;
    }
    if decisions.first() == Some(&Some(PermissionDecision::Allow)) {
        return ProfileDecision::Allowed;
    }
    ProfileDecision::Unspecified
}
fn strings(value: &serde_json::Value, output: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => output.push(s.clone()),
        serde_json::Value::Array(values) => {
            for v in values {
                strings(v, output);
            }
        }
        serde_json::Value::Object(values) => {
            for v in values.values() {
                strings(v, output);
            }
        }
        _ => {}
    }
}
fn normalized_path(path: &str) -> String {
    let expanded = if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map_or_else(
                || path.to_owned(),
                |home| {
                    std::path::PathBuf::from(home)
                        .join(rest)
                        .to_string_lossy()
                        .into_owned()
                },
            )
    } else {
        path.to_owned()
    };
    let crate::Resource::Path(path) = crate::Resource::path(expanded) else {
        unreachable!()
    };
    path.to_string_lossy().replace('\\', "/")
}
impl PermissionProfile {
    #[must_use]
    pub fn has_network_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|r| matches!(r.matcher, RuleMatcher::Domain { .. }))
    }
    #[must_use]
    pub fn network_decision(&self, url: &reqwest::Url) -> Option<PermissionDecision> {
        let host = url.host_str()?;
        let mut decision = None;
        for rule in &self.rules {
            if let RuleMatcher::Domain { pattern } = &rule.matcher
                && glob(&pattern.to_ascii_lowercase(), &host.to_ascii_lowercase())
            {
                decision = Some(match (decision, rule.decision) {
                    (Some(PermissionDecision::Deny), _) | (_, PermissionDecision::Deny) => {
                        PermissionDecision::Deny
                    }
                    (Some(PermissionDecision::Ask), _) | (_, PermissionDecision::Ask) => {
                        PermissionDecision::Ask
                    }
                    _ => PermissionDecision::Allow,
                });
            }
        }
        decision
    }

    /// Overlapping rules resolve deny > ask > allow, regardless of file order.
    #[must_use]
    pub fn decision(
        &self,
        tool: &str,
        input: &serde_json::Value,
        capability: Capability,
        resources: &[crate::ResourceAccess],
    ) -> Option<PermissionDecision> {
        let process = matches!(
            capability,
            Capability::Shell | Capability::Process | Capability::Mcp
        );
        if (self.boundary.read_only && (process || resources.iter().any(|r| r.write)))
            || (self.boundary.deny_network && (process || capability == Capability::Network))
        {
            return Some(PermissionDecision::Deny);
        }
        let mut values = Vec::new();
        strings(input, &mut values);
        let mut result = None;
        for rule in &self.rules {
            let matches = match &rule.matcher {
                RuleMatcher::Command { pattern } => {
                    capability == Capability::Shell
                        && input["command"].as_str().is_some_and(|c| glob(pattern, c))
                }
                RuleMatcher::Prefix { value } => {
                    capability == Capability::Shell
                        && input["command"].as_str().is_some_and(|c| {
                            c == value || c.strip_prefix(value).is_some_and(|s| s.starts_with(' '))
                        })
                }
                RuleMatcher::Path { pattern } => {
                    let normalized = normalized_path(pattern);
                    let subtree = normalized
                        .strip_suffix("/**")
                        .map(|prefix| prefix.trim_end_matches('/').to_owned());
                    resources.iter().any(|access| match &access.resource {
                        crate::Resource::Path(path) => {
                            let candidate = normalized_path(&path.to_string_lossy());
                            subtree.as_ref().map_or_else(
                                || path_glob(&normalized, &candidate),
                                |prefix| {
                                    candidate == *prefix
                                        || (rule.decision != PermissionDecision::Allow
                                            && candidate
                                                .strip_prefix(prefix.as_str())
                                                .is_some_and(|rest| rest.starts_with('/')))
                                },
                            )
                        }
                        crate::Resource::All => rule.decision != PermissionDecision::Allow,
                        crate::Resource::Named(_) => false,
                    })
                    // Arbitrary subprocess/MCP effects cannot be inferred from arguments.
                    || (process && rule.decision != PermissionDecision::Allow)
                }
                RuleMatcher::Domain { pattern } => {
                    values.iter().any(|v| {
                        reqwest::Url::parse(v)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_owned))
                            .is_some_and(|host| {
                                glob(&pattern.to_ascii_lowercase(), &host.to_ascii_lowercase())
                            })
                    }) || (process && rule.decision != PermissionDecision::Allow)
                }
                RuleMatcher::ToolParameter {
                    tool: name,
                    pointer,
                    pattern,
                } => {
                    glob(name, tool)
                        && input
                            .pointer(pointer)
                            .is_some_and(|v| glob(pattern, v.as_str().unwrap_or(&v.to_string())))
                }
            };
            if matches {
                result = Some(match (result, rule.decision) {
                    (Some(PermissionDecision::Deny), _) | (_, PermissionDecision::Deny) => {
                        PermissionDecision::Deny
                    }
                    (Some(PermissionDecision::Ask), _) | (_, PermissionDecision::Ask) => {
                        PermissionDecision::Ask
                    }
                    _ => PermissionDecision::Allow,
                });
            }
        }
        // An allow for a command prefix cannot authorize compound shell syntax.
        if result == Some(PermissionDecision::Allow)
            && capability == Capability::Shell
            && input["command"].as_str().is_some_and(|c| {
                c.contains([';', '|', '&', '`', '$', '%', '<', '>', '(', ')', '\n', '\r'])
            })
        {
            return Some(PermissionDecision::Ask);
        }
        result
    }
}
#[cfg(test)]
mod rule_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn precedence_and_parameter_rules() {
        let profile = PermissionProfile {
            rules: vec![
                PermissionRule {
                    decision: PermissionDecision::Allow,
                    matcher: RuleMatcher::Command {
                        pattern: "cargo *".into(),
                    },
                },
                PermissionRule {
                    decision: PermissionDecision::Ask,
                    matcher: RuleMatcher::Prefix {
                        value: "cargo publish".into(),
                    },
                },
                PermissionRule {
                    decision: PermissionDecision::Deny,
                    matcher: RuleMatcher::ToolParameter {
                        tool: "shell".into(),
                        pointer: "/command".into(),
                        pattern: "*secret*".into(),
                    },
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            profile.decision(
                "shell",
                &json!({"command":"cargo build"}),
                Capability::Shell,
                &[]
            ),
            Some(PermissionDecision::Allow)
        );
        assert_eq!(
            profile.decision(
                "shell",
                &json!({"command":"cargo publish secret"}),
                Capability::Shell,
                &[]
            ),
            Some(PermissionDecision::Deny)
        );
        assert_eq!(
            profile.decision(
                "shell",
                &json!({"command":"cargo build; evil"}),
                Capability::Shell,
                &[]
            ),
            Some(PermissionDecision::Ask)
        );
    }
    #[test]
    fn child_cannot_override_parent_deny_and_paths_normalize() {
        let parent = PermissionProfile {
            rules: vec![PermissionRule {
                decision: PermissionDecision::Deny,
                matcher: RuleMatcher::Path {
                    pattern: std::env::temp_dir()
                        .join("blocked")
                        .to_string_lossy()
                        .to_string()
                        + "/**",
                },
            }],
            ..Default::default()
        };
        let child = PermissionProfile {
            rules: vec![PermissionRule {
                decision: PermissionDecision::Allow,
                matcher: RuleMatcher::Command {
                    pattern: "*".into(),
                },
            }],
            ..Default::default()
        };
        let call = |command: &str| json!({"command":command});
        // Parent profile first: the child's Allow still cannot widen it.
        assert_eq!(
            resolve_profiles(&[
                parent.decision("shell", &call("cargo build"), Capability::Shell, &[]),
                child.decision("shell", &call("cargo build"), Capability::Shell, &[]),
            ]),
            ProfileDecision::Deny
        );
        // A later Allow is never the leading allowance that skips approval.
        assert_eq!(
            resolve_profiles(&[
                None,
                child.decision("shell", &call("cargo build"), Capability::Shell, &[]),
            ]),
            ProfileDecision::Unspecified
        );
        assert_eq!(
            resolve_profiles(&[
                child.decision("shell", &call("cargo build"), Capability::Shell, &[]),
                parent.decision("shell", &call("cargo build"), Capability::Shell, &[]),
            ]),
            ProfileDecision::Deny
        );
        assert_eq!(
            parent.decision(
                "filesystem",
                &json!({}),
                Capability::FilesystemRead,
                &[crate::ResourceAccess::read(crate::Resource::path(
                    std::env::temp_dir().join("allowed/../blocked/key")
                ))]
            ),
            Some(PermissionDecision::Deny)
        );
    }
    #[test]
    fn a_trailing_star_stays_inside_one_path_segment() {
        let directory = std::env::temp_dir().join("ax-glob-segment");
        let profile = PermissionProfile {
            rules: vec![PermissionRule {
                decision: PermissionDecision::Allow,
                matcher: RuleMatcher::Path {
                    pattern: format!("{}/*", directory.to_string_lossy()),
                },
            }],
            ..Default::default()
        };
        let decide = |path: std::path::PathBuf| {
            profile.decision(
                "filesystem",
                &json!({}),
                Capability::FilesystemRead,
                &[crate::ResourceAccess::read(crate::Resource::path(path))],
            )
        };
        assert_eq!(
            decide(directory.join("file.txt")),
            Some(PermissionDecision::Allow)
        );
        // A sibling directory that merely shares the prefix must not be covered.
        let sibling = std::path::PathBuf::from(format!("{}-other", directory.to_string_lossy()));
        assert_ne!(
            decide(sibling.join("file.txt")),
            Some(PermissionDecision::Allow)
        );
        // Nor a nested path below a direct child.
        assert_ne!(
            decide(directory.join("nested").join("file.txt")),
            Some(PermissionDecision::Allow)
        );
    }
}
