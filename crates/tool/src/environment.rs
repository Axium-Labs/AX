//! Compact, cached runtime capabilities shared by every kernel binding.
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

#[derive(Clone, Debug, Serialize)]
pub struct Executable {
    pub path: PathBuf,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvironmentContext {
    pub os: &'static str,
    pub shell: &'static str,
    pub shell_contract: &'static str,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub path_separator: char,
    /// Executable path/version probes. Empty (and omitted) in the lightweight
    /// context, which never spawns a probe process.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub executables: BTreeMap<String, Executable>,
    pub network_policy: &'static str,
    pub sandbox: String,
    pub write_boundary: PathBuf,
}

impl EnvironmentContext {
    /// Bounded runtime context: cwd, workspace root, sandbox/network posture and
    /// the shell contract, without executable version probes.
    ///
    /// This is cheap and safe to inject on every run, including a direct answer:
    /// it describes the environment so the model can use it when the request
    /// needs it. It does not ask the model to inspect or change anything, so its
    /// presence never triggers an action.
    #[must_use]
    pub fn light(cwd: &Path, root: &Path) -> Self {
        Self::base(cwd, root, BTreeMap::new())
    }

    /// Full runtime context including cached executable path/version probes.
    /// Probing spawns one short-lived process per known executable, so it is
    /// reserved for callers that actually need process capabilities.
    #[must_use]
    pub fn detect(cwd: &Path, root: &Path) -> Self {
        static CAPABILITIES: OnceLock<BTreeMap<String, Executable>> = OnceLock::new();
        let executables = CAPABILITIES
            .get_or_init(|| {
                [
                    "git", "python", "python3", "pip", "uv", "conda", "cargo", "node", "npm",
                ]
                .into_iter()
                .filter_map(|name| {
                    let path = locate(name)?;
                    let version = version(&path);
                    Some((name.to_owned(), Executable { path, version }))
                })
                .collect()
            })
            .clone();
        Self::base(cwd, root, executables)
    }

    fn base(cwd: &Path, root: &Path, executables: BTreeMap<String, Executable>) -> Self {
        Self {
            os: std::env::consts::OS,
            shell: if cfg!(windows) {
                "Windows PowerShell 5.1 (powershell.exe)"
            } else {
                "POSIX sh"
            },
            shell_contract: if cfg!(windows) {
                "Use PowerShell here-strings or python -c; no bash heredoc, && or ||. Use separate commands and check $LASTEXITCODE."
            } else {
                "Use POSIX sh syntax."
            },
            cwd: cwd.to_owned(),
            workspace_root: root.to_owned(),
            path_separator: std::path::MAIN_SEPARATOR,
            executables,
            network_policy: "sandbox allows network; tool permissions still apply",
            sandbox: format!(
                "{:?}",
                sandbox::SandboxManager::configured_mode().unwrap_or_default()
            ),
            write_boundary: root.to_owned(),
        }
    }
}

fn locate(name: &str) -> Option<PathBuf> {
    let extensions = if cfg!(windows) {
        vec![".exe", ".cmd", ".bat"]
    } else {
        vec![""]
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        extensions
            .iter()
            .map(|extension| directory.join(format!("{name}{extension}")))
            .find(|path| path.is_file())
    })
}

fn version(path: &Path) -> Option<String> {
    use std::process::{Command, Stdio};
    // Do not execute shell scripts or Windows Store aliases to probe capabilities.
    if path
        .extension()
        .is_some_and(|extension| extension == "cmd" || extension == "bat")
        || path.to_string_lossy().contains("WindowsApps")
    {
        return None;
    }
    let mut child = Command::new(path)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if started.elapsed() > std::time::Duration::from_secs(2) {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .map(str::to_owned)
}
