//! Shared file-discovery policy for `find_files`/`glob` and `search`.
//!
//! Both tools must see exactly the same tree, so traversal policy lives here
//! instead of in either tool: one default skip set, one ignore-file policy, one
//! symlink rule and one glob engine. Only the per-entry action differs.
//!
//! Traversal is provided by the `ignore` crate — the same walker ripgrep uses —
//! so AX never depends on a user-installed `rg` binary.

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use serde_json::Value;

use crate::ToolError;

/// Generated, vendored and cache directories discovery never descends into.
/// Shared by both tools so a filename scan and a content scan of one root can
/// never disagree about which files exist.
pub(crate) const DEFAULT_SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "dist",
    "build",
    "__pycache__",
    ".cache",
    "cache",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".next",
    ".turbo",
    ".gradle",
    "coverage",
    "vendor",
    ".ax",
    "child-runs",
];

/// Largest file content search will read. Larger files are skipped by the
/// walker itself, before any bytes are loaded.
pub(crate) const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn is_skipped_dir(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| DEFAULT_SKIP_DIRS.contains(&name))
}

/// A configured, reuse-safe walker for one root.
pub(crate) fn walker(root: &Path, max_depth: Option<usize>) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        // Hidden *files* stay discoverable (`.env`, `.github/workflows`); the
        // explicit skip set, not the hidden bit, is the policy.
        .hidden(false)
        // A link must never widen the bound workspace or create a cycle.
        .follow_links(false)
        // Honour `.gitignore` even when the root is not a git checkout, and
        // keep the child-workspace ignore file meaningful for both tools.
        .require_git(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .add_custom_ignore_filename(".axignore")
        .max_depth(max_depth)
        .filter_entry(|entry| {
            !entry.file_type().is_some_and(|kind| kind.is_dir())
                || !is_skipped_dir(entry.file_name())
        });
    builder
}

/// One compiled include/exclude pair.
///
/// Patterns are matched against both the root-relative path and the bare file
/// name, so `*.rs` works at any depth while `src/*.rs` stays depth-exact.
pub(crate) struct Globs {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl Globs {
    pub(crate) fn compile(include: &[String], exclude: &[String]) -> Result<Self, ToolError> {
        Ok(Self {
            include: compile_set(include)?,
            exclude: compile_set(exclude)?,
        })
    }

    pub(crate) fn allows(&self, relative: &str, name: &str) -> bool {
        if let Some(exclude) = &self.exclude
            && (exclude.is_match(relative) || exclude.is_match(name))
        {
            return false;
        }
        match &self.include {
            Some(include) => include.is_match(relative) || include.is_match(name),
            None => true,
        }
    }
}

fn compile_set(patterns: &[String]) -> Result<Option<GlobSet>, ToolError> {
    let patterns: Vec<&str> = patterns
        .iter()
        .map(|pattern| pattern.trim().trim_start_matches("./"))
        .filter(|pattern| !pattern.is_empty())
        .collect();
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        // `literal_separator` keeps `*` inside one path segment, so `src/*.rs`
        // does not also match `src/nested/a.rs`; `**` remains the recursive
        // form and the bare file name is matched separately.
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|error| {
                    ToolError::InvalidInput(format!("invalid glob {pattern:?}: {error}"))
                })?,
        );
    }
    builder
        .build()
        .map(Some)
        .map_err(|error| ToolError::InvalidInput(format!("invalid glob set: {error}")))
}

/// Accepts one string or an array of strings, so a single pattern stays terse
/// while a batch needs no extra call.
pub(crate) fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// The directory a call is scoped to: `path` is the canonical argument, `root`
/// is the alias. A relative argument is joined onto `workspace` lexically, so
/// `.` disappears and `..` can never climb above the bound workspace; an
/// absolute argument is taken as-is and left to the runtime scope check.
pub(crate) fn scope_root(workspace: &Path, path: Option<&str>, root: Option<&str>) -> PathBuf {
    let Some(raw) = path.or(root).filter(|raw| !raw.is_empty()) else {
        return workspace.to_path_buf();
    };
    let raw = Path::new(raw);
    if raw.is_absolute() {
        return raw.to_path_buf();
    }
    let mut scope = workspace.to_path_buf();
    for component in raw.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                scope.pop();
            }
            Component::Normal(part) => scope.push(part),
            other => scope.push(other.as_os_str()),
        }
    }
    scope
}

/// Root-relative, `/`-separated path, or `None` when `path` is not under
/// `root`. `..` and `.` components are dropped, so a returned path can never
/// climb out of the root it was made relative to.
pub(crate) fn strip(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut segments = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => segments.push(segment.to_string_lossy().into_owned()),
            Component::CurDir => {}
            // A `..` only *looks* rooted here; as a relative path it could
            // climb back out, so it is not a usable result.
            _ => return None,
        }
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

/// Path shown to the model for one result: workspace-relative when possible so
/// it can be passed straight back to `filesystem.read`, otherwise relative to
/// the requested scope, otherwise absolute.
pub(crate) fn result_path(workspace: &Path, scope: &Path, path: &Path) -> String {
    strip(workspace, path)
        .or_else(|| strip(scope, path))
        .unwrap_or_else(|| path.display().to_string())
}

/// Readable form of the searched scope relative to the bound workspace.
pub(crate) fn display_scope(workspace: &Path, scope: &Path) -> String {
    if scope == workspace {
        ".".to_owned()
    } else {
        strip(workspace, scope).unwrap_or_else(|| scope.display().to_string())
    }
}

/// Number of leading bytes inspected for a NUL byte before a file is treated as
/// binary and skipped.
pub(crate) const BINARY_PROBE_BYTES: usize = 8 * 1024;

pub(crate) fn looks_binary(bytes: &[u8]) -> bool {
    bytes
        .get(..BINARY_PROBE_BYTES)
        .unwrap_or(bytes)
        .contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_bare_names_at_any_depth_and_paths_exactly() {
        let globs = Globs::compile(&["*.rs".into()], &["**/generated/**".into()]).unwrap();
        assert!(globs.allows("src/main.rs", "main.rs"));
        assert!(globs.allows("main.rs", "main.rs"));
        assert!(!globs.allows("src/main.toml", "main.toml"));

        let globs = Globs::compile(&["src/*.rs".into()], &[]).unwrap();
        assert!(globs.allows("src/main.rs", "main.rs"));
        assert!(!globs.allows("src/nested/main.rs", "main.rs"));

        let globs = Globs::compile(&["**/*.jsonl".into()], &[]).unwrap();
        assert!(globs.allows("axout/session.jsonl", "session.jsonl"));
        assert!(globs.allows("session.jsonl", "session.jsonl"));
        assert!(!globs.allows("session.json", "session.json"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let globs = Globs::compile(&["**/*.rs".into()], &["target/**".into()]).unwrap();
        assert!(globs.allows("crates/tool/src/lib.rs", "lib.rs"));
        assert!(!globs.allows("target/debug/build.rs", "build.rs"));
    }

    #[test]
    fn relative_paths_are_normalized_and_cannot_escape_the_root() {
        let root = Path::new("/workspace");
        assert_eq!(
            strip(root, Path::new("/workspace/src/main.rs")).as_deref(),
            Some("src/main.rs")
        );
        // A path outside the root is not made relative to it at all, so a
        // returned path can never climb back out with `..`.
        for outside in ["/outside/secret.txt", "/workspace/../secret.txt"] {
            assert_eq!(strip(root, Path::new(outside)), None);
        }
        assert_eq!(
            result_path(
                root,
                Path::new("/workspace/src"),
                Path::new("/workspace/src/a.rs")
            ),
            "src/a.rs"
        );
        // Outside the workspace the result stays usable relative to the scope.
        assert_eq!(
            result_path(root, Path::new("/elsewhere"), Path::new("/elsewhere/a.rs")),
            "a.rs"
        );
    }

    #[test]
    fn scope_display_is_workspace_relative() {
        let workspace = Path::new("/workspace");
        assert_eq!(display_scope(workspace, workspace), ".");
        assert_eq!(
            display_scope(workspace, Path::new("/workspace/out/sessions")),
            "out/sessions"
        );
    }

    #[test]
    fn default_skip_set_covers_generated_and_cache_directories() {
        for name in [
            ".git",
            "target",
            "node_modules",
            ".venv",
            "dist",
            "build",
            "__pycache__",
            ".cache",
            "cache",
        ] {
            assert!(is_skipped_dir(OsStr::new(name)), "{name} must be skipped");
        }
        assert!(!is_skipped_dir(OsStr::new("src")));
        assert!(!is_skipped_dir(OsStr::new("builder")));
    }

    #[test]
    fn scope_root_prefers_path_and_falls_back_to_the_workspace() {
        let workspace = Path::new("/workspace");
        assert_eq!(scope_root(workspace, None, None), workspace);
        assert_eq!(scope_root(workspace, Some(""), None), workspace);
        assert_eq!(
            scope_root(workspace, Some("src"), None),
            workspace.join("src")
        );
        assert_eq!(
            scope_root(workspace, None, Some("src")),
            workspace.join("src")
        );
        // `.` collapses so the scope compares equal to the workspace.
        assert_eq!(scope_root(workspace, Some("."), None), workspace);
        assert_eq!(
            scope_root(workspace, Some("src/./nested"), None),
            workspace.join("src/nested")
        );
        // `..` is resolved lexically and cannot climb above the workspace.
        assert_eq!(
            scope_root(workspace, Some("../.."), None),
            PathBuf::from("/")
        );
        assert_eq!(
            scope_root(workspace, Some("src/../lib"), None),
            workspace.join("lib")
        );
        // An already-bound absolute path must not be re-rooted.
        let absolute = std::env::temp_dir().join("ax-scope-absolute");
        assert_eq!(
            scope_root(workspace, Some(&absolute.to_string_lossy()), None),
            absolute
        );
    }
}
