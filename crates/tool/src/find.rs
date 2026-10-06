//! Filename, path and extension discovery. Never reads file contents.
use crate::discovery::{Globs, display_scope, result_path, scope_root, string_list, walker};
use crate::{Capability, Resource, ResourceAccess, SafetyLevel, Tool, ToolError};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const DEFAULT_MAX_RESULTS: usize = 200;
const MAX_MAX_RESULTS: usize = 2_000;

const DESCRIPTION: &str = "Find files and directories by name, path or extension without reading their contents. Use it when you do not know where a file lives; read the returned paths directly afterwards. `pattern` accepts globs such as `**/*.jsonl`, `src/**/*.ts` or `*.toml` (a bare name pattern matches at any depth). Supports include/exclude globs, depth and result limits. Generated directories (.git, target, node_modules, .venv, dist, build, caches) are skipped automatically. Returns workspace-relative paths. Prefer this over a recursive shell scan (Get-ChildItem -Recurse, find, rg --files); shell discovery is also appropriate when explicitly requested, dedicated tools are unavailable, or native filters/pipelines are needed.";

/// One tool, two accepted names. `find_files` and `glob` share this
/// implementation, schema and traversal so the model can pick either name
/// without a second code path.
///
/// The tool carries the workspace it is bound to. A child gets the same struct
/// with `workspace` rebound to `context.cwd`, so isolation is a value change,
/// not a second implementation.
pub struct FindFilesTool {
    name: &'static str,
    workspace: PathBuf,
}

impl FindFilesTool {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            name: "find_files",
            workspace,
        }
    }

    /// The `glob` alias, registered alongside `find_files`.
    #[must_use]
    pub fn glob(workspace: PathBuf) -> Self {
        Self {
            name: "glob",
            workspace,
        }
    }
}

impl Default for FindFilesTool {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_default())
    }
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    File,
    Dir,
    Any,
}

impl EntryKind {
    fn accepts(self, is_dir: bool) -> bool {
        match self {
            Self::File => !is_dir,
            Self::Dir => is_dir,
            Self::Any => true,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    include: Option<Value>,
    #[serde(default)]
    exclude: Option<Value>,
    #[serde(default = "default_kind", rename = "type")]
    kind: EntryKind,
    #[serde(default)]
    max_depth: Option<usize>,
    #[serde(default = "default_max_results")]
    max_results: usize,
}

const fn default_kind() -> EntryKind {
    EntryKind::File
}

const fn default_max_results() -> usize {
    DEFAULT_MAX_RESULTS
}

struct Plan {
    workspace: PathBuf,
    root: PathBuf,
    globs: Globs,
    kind: EntryKind,
    max_depth: Option<usize>,
    max_results: usize,
}

struct Outcome {
    paths: Vec<String>,
    truncated: bool,
    skipped: usize,
}

#[async_trait]
impl Tool for FindFilesTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::WorkspaceWorker
    }
    fn fork_for_run(&self, context: &crate::RunContext) -> Option<std::sync::Arc<dyn Tool>> {
        // Same struct, same algorithm, same schema; only the bound workspace
        // moves to the child's cwd.
        Some(std::sync::Arc::new(Self {
            name: self.name,
            workspace: context.cwd.clone(),
        }))
    }
    fn recursive_search(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        DESCRIPTION
    }
    fn guidance(&self) -> Option<&'static str> {
        Some(
            "find_files / glob: discover names, paths and extensions without reading contents. \
             Emit every independent discovery call for one work phase in the same response instead \
             of alternating search -> model -> read -> model. Read returned paths directly; never \
             repeat the same pattern over the same scope in one round. Prefer this over a recursive \
             shell scan when it can express the request.",
        )
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type":"string","description":"Directory (or single file) to search, relative to the workspace root. Defaults to the workspace root."},
                "root": {"type":"string","description":"Alias for `path`."},
                "pattern": {"type":"string","description":"Glob such as `**/*.jsonl`, `src/**/*.ts` or `*.toml`. Omit to list every entry."},
                "include": {"type":["string","array"],"items":{"type":"string"},"description":"Extra globs a path must match."},
                "exclude": {"type":["string","array"],"items":{"type":"string"},"description":"Globs a path must not match; wins over include."},
                "type": {"type":"string","enum":["file","dir","any"],"description":"Entry kind to return. Defaults to `file`."},
                "max_depth": {"type":"integer","minimum":0,"maximum":64,"description":"Levels below `path` to descend."},
                "max_results": {"type":"integer","minimum":1,"maximum":2000,"description":"Result cap. Defaults to 200."}
            },
            "required": [],
            "additionalProperties": false
        })
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    /// Discovery only reads the tree it is scoped to, so independent calls in
    /// one round share the read lease instead of taking a global write lock.
    fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
        let root = scope_root(
            &self.workspace,
            input["path"].as_str(),
            input["root"].as_str(),
        );
        vec![ResourceAccess::read(Resource::path(root))]
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        if input.max_results == 0 || input.max_results > MAX_MAX_RESULTS {
            return Err(ToolError::InvalidInput(format!(
                "max_results must be 1..={MAX_MAX_RESULTS}"
            )));
        }
        let mut include = string_list(input.include.as_ref());
        if let Some(pattern) = input.pattern.as_deref()
            && !pattern.trim().is_empty()
        {
            include.push(pattern.to_owned());
        }
        let plan = Plan {
            workspace: self.workspace.clone(),
            root: scope_root(
                &self.workspace,
                input.path.as_deref(),
                input.root.as_deref(),
            ),
            globs: Globs::compile(&include, &string_list(input.exclude.as_ref()))?,
            kind: input.kind,
            max_depth: input.max_depth,
            max_results: input.max_results,
        };
        let root = display_scope(&plan.workspace, &plan.root);
        let outcome = tokio::task::spawn_blocking(move || collect(&plan))
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        Ok(json!({
            "root": root,
            "paths": outcome.paths,
            "count": outcome.paths.len(),
            "truncated": outcome.truncated,
            "skipped": outcome.skipped,
        })
        .to_string())
    }
}

/// One bounded, ignore-aware traversal. Metadata only: no file is opened.
fn collect(plan: &Plan) -> Outcome {
    let mut outcome = Outcome {
        paths: Vec::new(),
        truncated: false,
        skipped: 0,
    };
    if let Ok(metadata) = std::fs::symlink_metadata(&plan.root) {
        if metadata.file_type().is_symlink() {
            outcome.skipped += 1;
            return outcome;
        }
        if metadata.is_file() {
            let name = file_name(&plan.root);
            if plan.kind.accepts(false) && plan.globs.allows(&name, &name) {
                outcome
                    .paths
                    .push(result_path(&plan.workspace, &plan.root, &plan.root));
            } else {
                outcome.skipped += 1;
            }
            return outcome;
        }
        if !metadata.is_dir() {
            outcome.skipped += 1;
            return outcome;
        }
    }
    for entry in walker(&plan.root, plan.max_depth).build() {
        let Ok(entry) = entry else {
            outcome.skipped += 1;
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(kind) = entry.file_type() else {
            outcome.skipped += 1;
            continue;
        };
        // A link could point outside the bound workspace; discovery stays inside.
        if kind.is_symlink() || !(kind.is_file() || kind.is_dir()) {
            outcome.skipped += 1;
            continue;
        }
        if !plan.kind.accepts(kind.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = result_path(&plan.workspace, &plan.root, entry.path());
        if !plan.globs.allows(&relative, &name) {
            outcome.skipped += 1;
            continue;
        }
        outcome.paths.push(relative);
        if outcome.paths.len() >= plan.max_results {
            outcome.truncated = true;
            break;
        }
    }
    outcome.paths.sort_unstable();
    outcome
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("ax-find-{}", uuid::Uuid::new_v4()));
        for dir in [
            "src/nested",
            "tests",
            "axout",
            "target/debug",
            "node_modules/pkg",
            ".venv/lib",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "src/main.rs",
            "src/nested/mod.rs",
            "tests/smoke.rs",
            "target/debug/build.rs",
            "node_modules/pkg/index.js",
            ".venv/lib/site.rs",
            "session.jsonl",
            "axout/events.jsonl",
            "notes.md",
        ] {
            std::fs::write(root.join(file), "content").unwrap();
        }
        root
    }

    /// The tool is bound to `root`; a call then searches the bound workspace.
    async fn run(root: &Path, mut input: Value) -> Value {
        if input.get("path").is_none() {
            input["path"] = json!(".");
        }
        let output = FindFilesTool::new(root.to_path_buf())
            .execute(input)
            .await
            .unwrap();
        serde_json::from_str(&output).unwrap()
    }

    #[tokio::test]
    async fn locates_jsonl_files_with_a_recursive_glob() {
        let root = fixture();
        let result = run(&root, json!({"pattern":"**/*.jsonl"})).await;
        let paths: Vec<&str> = result["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["axout/events.jsonl", "session.jsonl"]);
        assert_eq!(result["truncated"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn skips_generated_directories_without_a_shell_scan() {
        let root = fixture();
        let result = run(&root, json!({"pattern":"**/*.rs"})).await;
        let paths: Vec<&str> = result["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            vec!["src/main.rs", "src/nested/mod.rs", "tests/smoke.rs"]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn supports_include_exclude_depth_kind_and_limits() {
        let root = fixture();
        let result = run(
            &root,
            json!({"include":["*.rs","*.js"],"exclude":["tests/**"],"max_results":2}),
        )
        .await;
        let paths: Vec<&str> = result["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|path| !path.starts_with("tests/")));
        assert_eq!(result["truncated"], true);

        let dirs = run(&root, json!({"type":"dir","max_depth":1})).await;
        let names: Vec<&str> = dirs["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["axout", "src", "tests"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn glob_alias_shares_the_same_schema_and_behavior() {
        let root = fixture();
        let glob = FindFilesTool::glob(root.clone());
        assert_eq!(glob.name(), "glob");
        assert_eq!(
            glob.input_schema(),
            FindFilesTool::new(root.clone()).input_schema()
        );
        assert_eq!(
            glob.description(),
            FindFilesTool::new(root.clone()).description()
        );
        let output = glob
            .execute(json!({"path":".","pattern":"*.jsonl"}))
            .await
            .unwrap();
        let result: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["count"], 2);
        assert_eq!(result["root"], ".");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_relative_scope_is_reported_relative_to_the_workspace() {
        let root = fixture();
        let result = run(&root, json!({"path":"axout","pattern":"**/*.jsonl"})).await;
        assert_eq!(result["root"], "axout");
        assert_eq!(result["paths"], json!(["axout/events.jsonl"]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_declares_read_only_resources() {
        let root = std::env::temp_dir().join("ax-find-resource");
        let access = FindFilesTool::new(root.clone()).resources(&json!({"path":"src"}));
        assert_eq!(access.len(), 1);
        assert!(!access[0].write);
        assert_eq!(access[0].resource, Resource::path(root.join("src")));
    }
}
