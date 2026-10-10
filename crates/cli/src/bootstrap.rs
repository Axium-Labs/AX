//! Process bootstrap: sandbox helpers, state locations, sandbox policy.
//!
//! The sandbox worker, broker and proxy entry points short-circuit before any
//! other work, so they own the process without touching provider, credential
//! or storage state. Everything else here only resolves *where* AX keeps its
//! state and *how* it is confined.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow};
use runtime_core::{AllowAll, ApprovalPolicy, DenyDangerous};

use crate::{
    args::Cli,
    config::{self, AxConfig},
    session_projects, storage_location, worktree_changes,
};

pub(crate) fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join("memory.sqlite3")
}
/// Recognized project markers, checked when no enclosing Git repository is
/// found. Intentionally small: this only needs to distinguish "a project
/// lives here" from an arbitrary directory, not identify the ecosystem.
const PROJECT_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "Gemfile",
    "composer.json",
];
/// Locates the stable project identity used for Project-scope memory,
/// independent of `--data-dir` (which only controls where AX stores state).
/// Prefers the enclosing Git repository, then a recognized project marker
/// file, and falls back to `start` itself so every invocation still resolves
/// to a concrete, stable path.
///
/// The search stops before the user's home directory so a dotfiles repo or a
/// stray marker file directly under `~` never turns the entire home
/// directory into one giant "project".
pub(crate) fn discover_project_root(start: &Path) -> PathBuf {
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let boundary = home_directory();
    let candidates = start
        .ancestors()
        .take_while(|dir| boundary.as_deref() != Some(*dir))
        .collect::<Vec<_>>();
    if let Some(root) = candidates.iter().find(|dir| dir.join(".git").exists()) {
        return (*root).to_path_buf();
    }
    if let Some(root) = candidates.iter().find(|dir| {
        dir.join(".ax/project.json").is_file()
            || dir.join(".ax/project-id").is_file()
            || storage_location::project_directory(dir)
                .join("project.json")
                .is_file()
    }) {
        return (*root).to_path_buf();
    }
    if let Some(root) = candidates.iter().find(|dir| {
        PROJECT_MARKERS
            .iter()
            .any(|marker| dir.join(marker).exists())
    }) {
        return (*root).to_path_buf();
    }
    start
}
pub(crate) fn resolve_directories(
    cwd: &Path,
    data_dir: Option<PathBuf>,
    skills_dir: Option<PathBuf>,
) -> (PathBuf, PathBuf) {
    let project_root = discover_project_root(cwd);
    (
        data_dir.unwrap_or_else(|| storage_location::project_directory(&project_root)),
        skills_dir.unwrap_or_else(|| project_root.join("skills")),
    )
}
fn home_directory() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .and_then(|home| fs::canonicalize(home).ok())
}
/// Global AX credential store, following pi's `~/.pi/agent/auth.json`
/// separation from project/session state.
pub(crate) fn ax_auth_path() -> PathBuf {
    config::ax_home().join("auth.json")
}
/// Global model catalogs, matching pi's single user-level model store rather
/// than duplicating provider discovery results in every project.
pub(crate) fn ax_models_dir() -> PathBuf {
    config::ax_home().join("models")
}
/// Older AX builds stored provider credentials inside the current project's
/// data directory. Preserve those logins once, but never import another
/// application's credentials (notably `~/.codex/auth.json`).
pub(crate) fn migrate_legacy_project_auth(data_dir: &Path) -> Result<()> {
    let legacy = data_dir.join("auth.json");
    let target = ax_auth_path();
    if target.exists() || !legacy.is_file() || legacy == target {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&legacy, &target).with_context(|| {
        format!(
            "failed to migrate AX credentials from {} to {}",
            legacy.display(),
            target.display()
        )
    })?;
    Ok(())
}
/// Sandbox worker, broker and proxy entry points. They are selected before
/// argument parsing, so no provider, credential or storage initialization runs
/// first, and they own the process for their lifetime.
pub(crate) async fn run_internal_entry(
    internal: Option<&str>,
    raw_args: &mut std::iter::Skip<std::env::Args>,
) -> Option<Result<()>> {
    let failure = |message: &str| Some(Err(anyhow!("{message}")));
    match internal? {
        #[cfg(windows)]
        "--ax-computer-use-worker" => match AxConfig::load() {
            Ok(config) => Some(tool::desktop_worker(config.computer_use).await.map_err(Into::into)),
            Err(error) => Some(Err(error)),
        },
        "--ax-sandbox-snapshot" => Some(worktree_changes::worker_snapshot()),
        "--ax-sandbox-worker" => match raw_args.next() {
            Some(name) => Some(tool::sandbox_worker(&name).await.map_err(Into::into)),
            None => failure("worker tool missing"),
        },
        #[cfg(target_os = "linux")]
        "--ax-sandbox-broker" => match raw_args.next() {
            Some(socket) => Some(
                sandbox::run_broker(PathBuf::from(socket))
                    .await
                    .map_err(Into::into),
            ),
            None => failure("broker socket missing"),
        },
        #[cfg(target_os = "linux")]
        "--ax-sandbox-proxy" => {
            let (Some(socket), Some(spec)) = (raw_args.next(), raw_args.next()) else {
                return failure("proxy socket or command missing");
            };
            match sandbox::run_proxy(PathBuf::from(socket), &spec).await {
                Ok(code) => std::process::exit(code),
                Err(error) => Some(Err(error.into())),
            }
        }
        _ => None,
    }
}

/// Where this invocation's state lives, and the directory it was launched from.
pub(crate) struct Locations {
    pub(crate) cwd: PathBuf,
    pub(crate) data_dir: PathBuf,
    pub(crate) skills_dir: PathBuf,
}

/// Confines the process and resolves its state locations. This is the whole of
/// the measured startup path: sandbox policy plus storage resolution.
///
/// # Errors
/// Returns an error when the sandbox cannot be configured or the state
/// directories cannot be initialized.
pub(crate) fn prepare(cli: &Cli) -> Result<Locations> {
    let cwd = std::env::current_dir()?;
    let mode = cli.sandbox.unwrap_or(AxConfig::load()?.sandbox);
    let protected = vec![
        config::ax_home()
            .canonicalize()
            .unwrap_or_else(|_| config::ax_home()),
    ];
    sandbox::SandboxManager::configure_workspaces(
        session_projects::list()?
            .into_iter()
            .map(|p| p.root.canonicalize().unwrap_or(p.root))
            .collect(),
    )?;
    sandbox::SandboxManager::configure(mode, protected)?;
    let (data_dir, skills_dir) =
        storage_location::initialize(&cwd, cli.data_dir.clone(), cli.skills_dir.clone())?;
    Ok(Locations {
        cwd,
        data_dir,
        skills_dir,
    })
}

/// Process-wide singletons that outlive any single command: the telemetry
/// sink, the credential store path and the approval policy.
pub(crate) struct Globals {
    pub(crate) auth_path: PathBuf,
    pub(crate) approval: Arc<dyn ApprovalPolicy>,
}

pub(crate) fn globals(cli: &Cli) -> Globals {
    model::stats::init(config::ax_home().join("inference_stats.json"));
    let approval: Arc<dyn ApprovalPolicy> = if cli.allow_dangerous {
        Arc::new(AllowAll)
    } else {
        Arc::new(DenyDangerous)
    };
    Globals {
        auth_path: ax_auth_path(),
        approval,
    }
}

#[cfg(test)]
mod project_root_tests {
    use super::{discover_project_root, resolve_directories};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ax-project-root-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn prefers_the_enclosing_git_repository_over_a_marker_file() {
        let root = temp_dir("git");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        let nested = root.join("crates").join("a");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            discover_project_root(&nested),
            fs::canonicalize(&root).unwrap()
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn falls_back_to_a_marker_file_without_git() {
        let root = temp_dir("marker");
        fs::write(root.join("package.json"), "{}").unwrap();
        let nested = root.join("src");
        fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            discover_project_root(&nested),
            fs::canonicalize(&root).unwrap()
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn falls_back_to_the_starting_directory_when_nothing_is_found() {
        let root = temp_dir("bare");

        assert_eq!(
            discover_project_root(&root),
            fs::canonicalize(&root).unwrap()
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn default_directories_are_stable_from_nested_working_directories() {
        let root = temp_dir("stable-defaults");
        fs::create_dir_all(root.join(".git")).unwrap();
        let nested = root.join("crates").join("core");
        fs::create_dir_all(&nested).unwrap();

        let from_root = resolve_directories(&root, None, None);
        let from_nested = resolve_directories(&nested, None, None);
        assert_eq!(from_root, from_nested);
        assert_eq!(
            from_root.0,
            crate::storage_location::project_directory(&fs::canonicalize(&root).unwrap())
        );
        assert_eq!(from_root.1, fs::canonicalize(&root).unwrap().join("skills"));

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn explicit_directories_override_project_defaults() {
        let root = temp_dir("explicit-defaults");
        fs::create_dir_all(root.join(".git")).unwrap();
        let data = PathBuf::from("custom-data");
        let skills = PathBuf::from("custom-skills");

        assert_eq!(
            resolve_directories(&root, Some(data.clone()), Some(skills.clone())),
            (data, skills)
        );

        fs::remove_dir_all(&root).unwrap();
    }
}
