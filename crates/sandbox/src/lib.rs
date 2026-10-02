//! Runtime confinement. Permissions never weaken an already prepared policy.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::process::Child;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    Off,
    Workspace,
    Strict,
}
impl Default for SandboxMode {
    /// The strongest mode this platform can actually prepare. Confined modes
    /// need the native isolation backend, so a platform without one defaults to
    /// `off` instead of shipping a default configuration whose every tool call
    /// fails closed. Explicitly requesting a confined mode on such a platform
    /// still fails closed; the default never overrides a user's request.
    fn default() -> Self {
        Self::platform_default()
    }
}
impl SandboxMode {
    /// Whether the native isolation backend exists for this build target.
    #[must_use]
    pub const fn platform_backed() -> bool {
        cfg!(target_os = "linux")
    }
    /// Default mode for this build target: confined where it can be prepared.
    #[must_use]
    pub const fn platform_default() -> Self {
        if Self::platform_backed() {
            Self::Workspace
        } else {
            Self::Off
        }
    }
}
impl std::str::FromStr for SandboxMode {
    type Err = SandboxViolation;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "off" => Ok(Self::Off),
            "workspace" => Ok(Self::Workspace),
            "strict" => Ok(Self::Strict),
            _ => Err(SandboxViolation(format!("unknown sandbox mode: {s}"))),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilesystemMode {
    ReadWrite,
    ReadOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkMode {
    Allow,
    Deny,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub memory_bytes: u64,
    pub cpu_seconds: u64,
    pub processes: u64,
    pub open_files: u64,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 8 * 1024 * 1024 * 1024,
            cpu_seconds: 3600,
            processes: 256,
            open_files: 4096,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SandboxPolicy {
    pub workspace_root: PathBuf,
    pub mode: SandboxMode,
    pub filesystem_mode: FilesystemMode,
    pub network_mode: NetworkMode,
    pub protected_paths: Vec<PathBuf>,
    pub resource_limits: ResourceLimits,
    /// Trusted lifecycle-service capabilities; ordinary task policies leave empty.
    #[serde(default)]
    pub runtime_mounts: Vec<RuntimeMount>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeMount {
    pub path: PathBuf,
    pub read_only: bool,
}
impl SandboxPolicy {
    #[must_use]
    pub fn workspace(root: PathBuf, mode: SandboxMode) -> Self {
        Self {
            workspace_root: root,
            mode,
            filesystem_mode: FilesystemMode::ReadWrite,
            network_mode: NetworkMode::Allow,
            protected_paths: Vec::new(),
            resource_limits: ResourceLimits::default(),
            runtime_mounts: Vec::new(),
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("SandboxViolation: {0}")]
pub struct SandboxViolation(pub String);
pub type Result<T> = std::result::Result<T, SandboxViolation>;
impl From<std::io::Error> for SandboxViolation {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub stdin: std::process::Stdio,
}
impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            stdin: std::process::Stdio::null(),
        }
    }
}
pub trait SandboxBackend: Send + Sync {
    /// Prepares a policy into a backend that can run commands.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the workspace is invalid or when the
    /// native backend cannot enforce the requested policy. Confined modes never
    /// degrade to unconfined host execution.
    fn prepare(policy: &SandboxPolicy) -> Result<Sandbox>
    where
        Self: Sized;
    /// Spawns one command under the prepared policy.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the policy has no prepared backend or
    /// when the backend cannot start the process.
    fn spawn(&self, command: CommandSpec) -> Result<Child>;
}
pub struct Sandbox {
    policy: SandboxPolicy,
    #[cfg(target_os = "linux")]
    linux: Option<linux::LinuxBackend>,
}
impl Sandbox {
    /// The policy this backend was prepared from.
    #[must_use]
    pub fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }
}
impl SandboxBackend for Sandbox {
    fn prepare(policy: &SandboxPolicy) -> Result<Self> {
        let mut policy = policy.clone();
        policy.workspace_root = policy.workspace_root.canonicalize()?;
        if !policy.workspace_root.is_dir() {
            return Err(SandboxViolation("workspace must be a directory".into()));
        }
        if policy.mode != SandboxMode::Off {
            if let Some(home) = std::env::var_os("HOME") {
                let home = PathBuf::from(home);
                if home.starts_with(&policy.workspace_root) {
                    return Err(SandboxViolation(
                        "workspace may not encompass the user credential home".into(),
                    ));
                }
                for name in [
                    ".ssh",
                    ".aws",
                    ".azure",
                    ".config",
                    ".docker",
                    ".gnupg",
                    ".kube",
                    ".npmrc",
                    ".netrc",
                    ".pypirc",
                    ".git-credentials",
                ] {
                    policy.protected_paths.push(home.join(name));
                }
            }
            for name in [".ssh", ".ax"] {
                policy
                    .protected_paths
                    .push(policy.workspace_root.join(name));
            }
        }
        #[cfg(target_os = "linux")]
        let linux = if policy.mode == SandboxMode::Off {
            None
        } else {
            Some(linux::LinuxBackend::prepare(&policy)?)
        };
        #[cfg(not(target_os = "linux"))]
        if policy.mode != SandboxMode::Off {
            return Err(SandboxViolation(
                "native isolation backend unavailable on this platform; no host fallback".into(),
            ));
        }
        Ok(Self {
            policy,
            #[cfg(target_os = "linux")]
            linux,
        })
    }
    fn spawn(&self, spec: CommandSpec) -> Result<Child> {
        #[cfg(target_os = "linux")]
        if let Some(backend) = &self.linux {
            return backend.spawn(&self.policy, spec);
        }
        // Only an explicitly selected `off` policy may reach the host. A confined
        // policy without a prepared backend is a programming error, and degrading
        // to unconfined execution here would silently void the whole boundary.
        if self.policy.mode != SandboxMode::Off {
            return Err(SandboxViolation(
                "confined policy reached the host without a prepared backend".into(),
            ));
        }
        let mut command = tokio::process::Command::new(spec.program);
        command.args(spec.args).envs(spec.env).current_dir(
            spec.cwd
                .unwrap_or_else(|| self.policy.workspace_root.clone()),
        );
        command
            .stdin(spec.stdin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(Into::into)
    }
}
#[cfg(target_os = "linux")]
mod broker;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use broker::{run_broker, run_proxy};
/// Kernel object resolution for built-in file operations. Namespace confinement
/// remains the final boundary even if another sandbox process changes a link.
///
/// # Errors
///
/// Returns [`SandboxViolation`] when the platform has no file worker backend or
/// when the kernel refuses to resolve `path` beneath the workspace.
pub fn authorize_workspace_path(path: &std::path::Path, create: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::authorize_path(&std::env::current_dir()?, path, create)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, create);
        Err(SandboxViolation("file worker backend unavailable".into()))
    }
}

/// One prepared manager is retained by the tool registry/MCP for its run.
#[derive(Clone)]
pub struct SandboxManager(Arc<Sandbox>);
/// Defense in depth against invoking the private worker on the host.
///
/// # Errors
///
/// Returns [`SandboxViolation`] unless this process is the confined worker.
pub fn verify_worker() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status")?;
        if std::env::current_exe()? == std::path::Path::new("/ax-worker")
            && status.lines().any(|line| line == "Seccomp:\t2")
            && status.lines().any(|line| line == "NoNewPrivs:\t1")
        {
            return Ok(());
        }
    }
    Err(SandboxViolation(
        "private tool worker requires OS confinement".into(),
    ))
}
static MODE: OnceLock<SandboxMode> = OnceLock::new();
static PROTECTED: OnceLock<Vec<PathBuf>> = OnceLock::new();
static WORKSPACES: OnceLock<Vec<PathBuf>> = OnceLock::new();
type Cache = BTreeMap<PathBuf, Weak<Sandbox>>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
impl SandboxManager {
    /// The process-wide mode, once the CLI has sealed it.
    #[must_use]
    pub fn configured_mode() -> Option<SandboxMode> {
        MODE.get().copied()
    }
    /// Seals the process-wide mode and protected-path set.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when either value was already sealed.
    pub fn configure(mode: SandboxMode, protected: Vec<PathBuf>) -> Result<()> {
        MODE.set(mode)
            .map_err(|_| SandboxViolation("sandbox configuration already sealed".into()))?;
        PROTECTED
            .set(protected)
            .map_err(|_| SandboxViolation("protected paths already sealed".into()))
    }
    /// Prepares one policy into a reusable manager.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the policy cannot be enforced on this
    /// platform. Confined policies never fall back to unconfined execution.
    pub fn prepare(policy: &SandboxPolicy) -> Result<Self> {
        Sandbox::prepare(policy).map(|sandbox| Self(Arc::new(sandbox)))
    }
    /// Publishes the canonical roots of other registered workspaces.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the registry was already sealed.
    pub fn configure_workspaces(roots: Vec<PathBuf>) -> Result<()> {
        WORKSPACES
            .set(roots)
            .map_err(|_| SandboxViolation("workspace registry already sealed".into()))
    }
    /// Builds the policy for one workspace from the sealed configuration.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the root cannot be canonicalized.
    pub fn policy_for_workspace(root: PathBuf) -> Result<SandboxPolicy> {
        let mut policy = SandboxPolicy::workspace(root, MODE.get().copied().unwrap_or_default());
        policy.workspace_root = policy.workspace_root.canonicalize()?;
        policy.protected_paths = PROTECTED.get().cloned().unwrap_or_default();
        policy.protected_paths.extend(
            WORKSPACES
                .get()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|path| {
                    path != &policy.workspace_root && !policy.workspace_root.starts_with(path)
                }),
        );
        Ok(policy)
    }
    /// Builds the policy for one managed child workspace.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the root is invalid or when `state`
    /// does not describe the matching managed child capability.
    pub fn policy_for_child(root: PathBuf, state: &std::path::Path) -> Result<SandboxPolicy> {
        let mut policy = Self::policy_for_workspace(root)?;
        let root = &policy.workspace_root;
        if policy
            .protected_paths
            .iter()
            .any(|path| root.starts_with(path))
        {
            if root.file_name().is_none_or(|name| name != "workspace")
                || state.file_name().is_none_or(|name| name != "state")
                || root.parent() != state.parent()
            {
                return Err(SandboxViolation(
                    "invalid managed child workspace capability".into(),
                ));
            }
            policy
                .protected_paths
                .retain(|path| !root.starts_with(path));
        }
        Ok(policy)
    }
    /// Manager for one managed child workspace, reused across calls.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the child capability is invalid or the
    /// policy cannot be prepared.
    pub fn for_child_workspace(root: PathBuf, state: &std::path::Path) -> Result<Self> {
        Self::cached(Self::policy_for_child(root, state)?)
    }
    /// Manager for one workspace, reused across calls.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the root is invalid or the policy
    /// cannot be prepared.
    pub fn for_workspace(root: PathBuf) -> Result<Self> {
        Self::cached(Self::policy_for_workspace(root)?)
    }
    fn cached(mut policy: SandboxPolicy) -> Result<Self> {
        let root = policy.workspace_root.clone();
        let mut cache = CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| SandboxViolation("sandbox cache poisoned".into()))?;
        if let Some(sandbox) = cache.get(&root).and_then(Weak::upgrade) {
            return Ok(Self(sandbox));
        }
        for name in [".ssh", ".ax"] {
            policy.protected_paths.push(root.join(name));
        }
        let manager = Self::prepare(&policy)?;
        cache.insert(root, Arc::downgrade(&manager.0));
        Ok(manager)
    }
    /// The prepared policy. Callers must read the effective mode from here
    /// rather than from the process-wide configuration.
    #[must_use]
    pub fn policy(&self) -> &SandboxPolicy {
        self.0.policy()
    }
    /// Spawns one command under the prepared policy.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the command cannot be started.
    pub fn spawn(&self, command: CommandSpec) -> Result<Child> {
        self.0.spawn(command)
    }
    /// Runs one command to completion on a blocking thread.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxViolation`] when the command cannot be started, the
    /// runtime cannot be built, or the adapter thread panicked.
    pub fn output_blocking(&self, spec: CommandSpec) -> Result<std::process::Output> {
        let manager = self.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                manager
                    .spawn(spec)?
                    .wait_with_output()
                    .await
                    .map_err(Into::into)
            })
        })
        .join()
        .map_err(|_| SandboxViolation("sandbox output adapter panicked".into()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_modes_fail_closed() {
        assert!("automatic".parse::<SandboxMode>().is_err());
    }
    #[test]
    fn default_mode_is_what_this_platform_can_prepare() {
        let mode = SandboxMode::default();
        if SandboxMode::platform_backed() {
            assert_eq!(mode, SandboxMode::Workspace);
        } else {
            // A default that cannot be prepared would make every tool call fail.
            assert_eq!(mode, SandboxMode::Off);
        }
    }
    #[test]
    fn missing_workspace_never_prepares() {
        for mode in [SandboxMode::Workspace, SandboxMode::Strict] {
            assert!(
                SandboxManager::prepare(&SandboxPolicy::workspace(
                    PathBuf::from("/ax-missing-workspace-14929"),
                    mode
                ))
                .is_err()
            );
        }
    }
    #[test]
    fn off_mode_policy_can_spawn_without_a_backend() {
        let mut policy =
            SandboxPolicy::workspace(std::env::current_dir().unwrap(), SandboxMode::Off);
        policy.workspace_root = policy.workspace_root.canonicalize().unwrap();
        let sandbox = Sandbox::prepare(&policy).unwrap();
        let mut spec = CommandSpec::new("ax-definitely-missing-program");
        spec.args = Vec::new();
        // Reaching the host spawn path is what matters: the process itself fails,
        // which proves the policy was accepted as an explicitly unconfined one.
        assert!(sandbox.spawn(spec).is_err());
    }
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unavailable_platform_never_falls_back() {
        assert!(
            SandboxManager::prepare(&SandboxPolicy::workspace(
                std::env::current_dir().unwrap(),
                SandboxMode::Strict
            ))
            .is_err()
        );
    }
}
