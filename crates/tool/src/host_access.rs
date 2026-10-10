//! Host UI access is independent of workspace confinement and capability grants.
use crate::{PermissionDecision, ToolError};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostSurface {
    Computer,
    Browser,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostTarget {
    pub surface: HostSurface,
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct HostAccessRequest {
    pub target: HostTarget,
    pub decision: PermissionDecision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostGrant {
    Deny,
    Once,
    Session,
    Always,
}

/// Produced by the scheduler after an independent host-access approval.
#[derive(Clone, Debug)]
pub struct HostAuthorization {
    target: HostTarget,
    grant: HostGrant,
}
impl HostAuthorization {
    /// # Errors
    /// Rejects an explicit denial or a denied approval choice.
    pub fn approved(request: &HostAccessRequest, grant: HostGrant) -> Result<Self, ToolError> {
        if request.decision == PermissionDecision::Deny || grant == HostGrant::Deny {
            return Err(ToolError::PermissionDenied(request.target.label.clone()));
        }
        Ok(Self {
            target: request.target.clone(),
            grant,
        })
    }
    #[must_use]
    pub fn target(&self) -> &HostTarget {
        &self.target
    }
    #[must_use]
    pub fn grant(&self) -> HostGrant {
        self.grant
    }
}

fn yes() -> bool {
    true
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostPermissions {
    pub browser_enabled: bool,
    pub apps: BTreeMap<String, PermissionDecision>,
    pub sites: BTreeMap<String, PermissionDecision>,
    /// Per-target epochs let separate CLI/UI processes revoke ephemeral grants.
    versions: BTreeMap<String, String>,
}
impl Default for HostPermissions {
    fn default() -> Self {
        Self {
            browser_enabled: yes(),
            apps: BTreeMap::new(),
            sites: BTreeMap::new(),
            versions: BTreeMap::new(),
        }
    }
}

type SessionGrants = BTreeMap<(PathBuf, String, HostSurface, String), (String, String)>;
fn epoch(state: &HostPermissions, target: &HostTarget) -> (String, String) {
    let key = format!("{:?}:{}", target.surface, target.id);
    (
        state.versions.get(&key).cloned().unwrap_or_default(),
        if target.surface == HostSurface::Browser {
            state.versions.get("browser").cloned().unwrap_or_default()
        } else {
            String::new()
        },
    )
}
fn sessions() -> &'static Mutex<SessionGrants> {
    static GRANTS: OnceLock<Mutex<SessionGrants>> = OnceLock::new();
    GRANTS.get_or_init(Mutex::default)
}
/// Called alongside capability-grant expiry when a new user session starts.
pub fn reset_host_session(scope: &str) {
    if let Ok(mut grants) = sessions().lock() {
        grants.retain(|(_, session, _, _), _| session != scope);
    }
    crate::browser::close_host_session(scope);
}

#[derive(Clone, Debug)]
pub struct HostPermissionStore {
    home: PathBuf,
    session: String,
}
impl HostPermissionStore {
    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            session: "direct".into(),
        }
    }
    #[must_use]
    pub fn with_session(mut self, session: &str) -> Self {
        self.session = session.into();
        self
    }
    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }
    /// # Errors
    /// Invalid or unreadable policy files fail closed.
    pub fn load(&self) -> Result<HostPermissions, ToolError> {
        match std::fs::read(self.home.join("host-permissions.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| ToolError::Execution(format!("Invalid host permissions: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HostPermissions::default()),
            Err(e) => Err(e.into()),
        }
    }
    /// # Errors
    /// Rejects unreadable policies or unavailable ephemeral grant storage.
    pub fn decision(&self, target: &HostTarget) -> Result<PermissionDecision, ToolError> {
        let state = self.load()?;
        if target.surface == HostSurface::Browser && !state.browser_enabled {
            return Ok(PermissionDecision::Deny);
        }
        let entries = match target.surface {
            HostSurface::Computer => &state.apps,
            HostSurface::Browser => &state.sites,
        };
        let decision = entries
            .get(&target.id)
            .copied()
            .unwrap_or(PermissionDecision::Ask);
        if decision != PermissionDecision::Ask {
            return Ok(decision);
        }
        let grants = sessions()
            .lock()
            .map_err(|_| ToolError::Execution("Host grants unavailable".into()))?;
        Ok(
            if grants.get(&(
                self.home.clone(),
                self.session.clone(),
                target.surface,
                target.id.clone(),
            )) == Some(&epoch(&state, target))
            {
                PermissionDecision::Allow
            } else {
                PermissionDecision::Ask
            },
        )
    }
    /// # Errors
    /// Rejects revocation, corrupt policy, unavailable storage or failed writes.
    pub fn grant(&self, authorization: &HostAuthorization) -> Result<(), ToolError> {
        let target = authorization.target();
        if self.decision(target)? == PermissionDecision::Deny {
            return Err(ToolError::PermissionDenied(target.label.clone()));
        }
        match authorization.grant() {
            HostGrant::Deny => return Err(ToolError::PermissionDenied(target.label.clone())),
            HostGrant::Once => {}
            HostGrant::Session => {
                let version = epoch(&self.load()?, target);
                sessions()
                    .lock()
                    .map_err(|_| ToolError::Execution("Host grants unavailable".into()))?
                    .insert(
                        (
                            self.home.clone(),
                            self.session.clone(),
                            target.surface,
                            target.id.clone(),
                        ),
                        version,
                    );
            }
            HostGrant::Always => {
                self.edit_inner(
                    Some(target),
                    Some(PermissionDecision::Allow),
                    false,
                    None,
                    true,
                )?;
            }
        }
        Ok(())
    }
    /// Serialize concurrent UI/approval writes without losing other app/site rules.
    /// # Errors
    /// Returns an error on policy corruption or locking/persistence failure.
    pub fn edit(
        &self,
        target: Option<&HostTarget>,
        decision: Option<PermissionDecision>,
        remove: bool,
        browser_enabled: Option<bool>,
    ) -> Result<HostPermissions, ToolError> {
        self.edit_inner(target, decision, remove, browser_enabled, false)
    }
    fn edit_inner(
        &self,
        target: Option<&HostTarget>,
        decision: Option<PermissionDecision>,
        remove: bool,
        browser_enabled: Option<bool>,
        grant_only: bool,
    ) -> Result<HostPermissions, ToolError> {
        use fs2::FileExt;
        use std::io::Write;
        std::fs::create_dir_all(&self.home)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.home.join("host-permissions.lock"))?;
        lock.lock_exclusive()?;
        let mut state = self.load()?;
        if grant_only && let Some(target) = target {
            let denied = match target.surface {
                HostSurface::Computer => {
                    state.apps.get(&target.id) == Some(&PermissionDecision::Deny)
                }
                HostSurface::Browser => {
                    !state.browser_enabled
                        || state.sites.get(&target.id) == Some(&PermissionDecision::Deny)
                }
            };
            if denied {
                return Err(ToolError::PermissionDenied(
                    "Host access was revoked while approval was pending".into(),
                ));
            }
        }
        if let Some(enabled) = browser_enabled {
            state.browser_enabled = enabled;
            state
                .versions
                .insert("browser".into(), uuid::Uuid::new_v4().to_string());
        }
        if let Some(target) = target {
            state.versions.insert(
                format!("{:?}:{}", target.surface, target.id),
                uuid::Uuid::new_v4().to_string(),
            );
            let entries = match target.surface {
                HostSurface::Computer => &mut state.apps,
                HostSurface::Browser => &mut state.sites,
            };
            if remove {
                entries.remove(&target.id);
            } else if let Some(decision) = decision {
                entries.insert(target.id.clone(), decision);
            }
            sessions()
                .lock()
                .map_err(|_| ToolError::Execution("Host grants unavailable".into()))?
                .retain(|(home, _, surface, id), _| {
                    home != &self.home || surface != &target.surface || id != &target.id
                });
        }
        let temporary = self
            .home
            .join(format!("host-permissions.{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<(), ToolError> {
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(
                &serde_json::to_vec_pretty(&state)
                    .map_err(|e| ToolError::Execution(e.to_string()))?,
            )?;
            file.sync_all()?;
            std::fs::rename(&temporary, self.home.join("host-permissions.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result?;
        Ok(state)
    }
}

pub fn host_home() -> PathBuf {
    std::env::var_os("AX_HOME").map_or_else(
        || {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf))
                .unwrap_or_default()
                .join(".ax")
        },
        PathBuf::from,
    )
}

/// # Errors
/// Rejects non-HTTP(S) URLs, missing hosts and embedded credentials.
pub fn browser_origin(value: &str) -> Result<String, ToolError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| ToolError::InvalidInput("Use an absolute http:// or https:// URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ToolError::InvalidInput(
            "Browser access accepts HTTP(S) origins without URL credentials".into(),
        ));
    }
    Ok(url.origin().ascii_serialization())
}

/// # Errors
/// Rejects relative application paths.
pub fn app_id(path: &Path) -> Result<String, ToolError> {
    if !path.is_absolute() {
        return Err(ToolError::InvalidInput(
            "Use the application's full executable path".into(),
        ));
    }
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let value = resolved.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let value = value.trim_start_matches("//?/").to_lowercase();
    Ok(value)
}

/// UI images use the existing multimodal channel, not a filesystem grant.
pub(crate) fn host_output(value: &serde_json::Value) -> Result<crate::ToolOutput, ToolError> {
    use base64::Engine;
    let description =
        serde_json::to_string(value).map_err(|e| ToolError::Execution(e.to_string()))?;
    let Some(path) = value["screenshot_path"].as_str() else {
        return Ok(crate::ToolOutput::Text(description));
    };
    let path = std::fs::canonicalize(path)?;
    if !path.starts_with(std::fs::canonicalize(std::env::temp_dir())?)
        || path.extension().is_none_or(|e| e != "png")
    {
        return Err(ToolError::PermissionDenied(
            "Unexpected host screenshot location".into(),
        ));
    }
    if std::fs::metadata(&path)?.len() > 32 * 1024 * 1024 {
        return Err(ToolError::Execution("Host image exceeds limit".into()));
    }
    let bytes = std::fs::read(&path)?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(ToolError::Execution("Invalid host PNG".into()));
    }
    Ok(crate::ToolOutput::Image {
        description,
        media_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}
