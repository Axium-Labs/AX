//! Content, symbol and regex search over the workspace.
//!
//! Traversal and matching use the `ignore` and `regex` crates — the same
//! building blocks ripgrep uses — instead of reading every file with
//! `read_to_string` and testing `contains`. AX carries the implementation, so
//! nothing depends on a user-installed `rg`.
use crate::discovery::{
    Globs, MAX_FILE_BYTES, display_scope, looks_binary, result_path, scope_root, string_list,
    walker,
};
use crate::{Capability, Resource, ResourceAccess, SafetyLevel, Tool, ToolError};
use async_trait::async_trait;
use ignore::{DirEntry, WalkState};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

const DEFAULT_MAX_RESULTS: usize = 100;
const MAX_MAX_RESULTS: usize = 2_000;
const MAX_CONTEXT_LINES: usize = 10;
const MAX_LINE_CHARS: usize = 500;
const MAX_CONTEXT_CHARS: usize = 200;

const DESCRIPTION: &str = "Search file contents for literal text, symbols or a regular expression. This is the grep tool: use find_files/glob for path-name discovery and filesystem read for full file context. Searching a known file is appropriate for targeted regex, symbol or usage analysis. `mode` is `literal` (default) or `regex`; matching is per line. Narrow the scan with `path`, `include` and `exclude` before widening it. `output_mode` is `content` (default) or `files_with_matches`. No match is a successful empty result, not a failure. Avoid repeating an unchanged query without new evidence; changed files, revised patterns/scopes or explicit user-requested verification can justify another search. Generated directories (.git, target, node_modules, .venv, dist, build, caches) are skipped automatically.";

/// Literal text or a regular expression.
#[derive(Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Mode {
    #[default]
    Literal,
    Regex,
}

#[derive(Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OutputMode {
    #[default]
    Content,
    FilesWithMatches,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    query: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    mode: Mode,
    #[serde(default = "default_case_sensitive")]
    case_sensitive: bool,
    #[serde(default)]
    include: Option<Value>,
    #[serde(default)]
    exclude: Option<Value>,
    #[serde(default)]
    context_lines: usize,
    #[serde(default = "default_max_results")]
    max_results: usize,
    #[serde(default)]
    output_mode: OutputMode,
    /// Accepted for compatibility with earlier callers; it never gates execution.
    #[serde(default, rename = "fallback_reason")]
    _fallback_reason: Option<String>,
}

const fn default_case_sensitive() -> bool {
    true
}

const fn default_max_results() -> usize {
    DEFAULT_MAX_RESULTS
}

/// Content search bound to one workspace.
///
/// A child agent gets this same struct with `workspace` rebound to
/// `context.cwd`: identical algorithm and schema, only the scope differs.
pub struct SearchTool {
    workspace: PathBuf,
}

impl SearchTool {
    #[must_use]
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

impl Default for SearchTool {
    fn default() -> Self {
        Self::new(std::env::current_dir().unwrap_or_default())
    }
}

struct Plan {
    workspace: PathBuf,
    root: PathBuf,
    pattern: String,
    case_sensitive: bool,
    globs: Globs,
    context_lines: usize,
    max_results: usize,
    output: OutputMode,
}

struct Hit {
    path: String,
    line: usize,
    text: String,
    before: Vec<String>,
    after: Vec<String>,
}

impl Hit {
    fn to_json(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("path".into(), json!(self.path));
        object.insert("line".into(), json!(self.line));
        object.insert("text".into(), json!(self.text));
        if !self.before.is_empty() {
            object.insert("before".into(), json!(self.before));
        }
        if !self.after.is_empty() {
            object.insert("after".into(), json!(self.after));
        }
        Value::Object(object)
    }
}

#[derive(Default)]
struct Collected {
    hits: Vec<Hit>,
    files: Vec<String>,
    scanned: usize,
    skipped: usize,
    truncated: bool,
}

#[async_trait]
impl Tool for SearchTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::WorkspaceWorker
    }
    fn fork_for_run(&self, context: &crate::RunContext) -> Option<std::sync::Arc<dyn Tool>> {
        // Same struct, same algorithm, same schema; only the bound workspace
        // moves to the child's cwd.
        Some(std::sync::Arc::new(Self::new(context.cwd.clone())))
    }
    fn recursive_search(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "search"
    }
    fn description(&self) -> &'static str {
        DESCRIPTION
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type":"string","description":"Text or pattern to find. A `literal` query is matched exactly; a `regex` query is a Rust regular expression matched per line."},
                "path": {"type":"string","description":"File or directory to search, relative to the workspace root. Defaults to the workspace root."},
                "root": {"type":"string","description":"Alias for `path`."},
                "mode": {"type":"string","enum":["literal","regex"],"description":"Matching mode. Defaults to `literal`."},
                "case_sensitive": {"type":"boolean","description":"Defaults to true; set false for case-insensitive matching."},
                "include": {"type":["string","array"],"items":{"type":"string"},"description":"Only search files matching these globs."},
                "exclude": {"type":["string","array"],"items":{"type":"string"},"description":"Skip files matching these globs; wins over include."},
                "context_lines": {"type":"integer","minimum":0,"maximum":10,"description":"Lines of context to return around each match. Defaults to 0."},
                "max_results": {"type":"integer","minimum":1,"maximum":2000,"description":"Result cap (match lines, or files with matches). Defaults to 100."},
                "output_mode": {"type":"string","enum":["content","files_with_matches"],"description":"`content` returns matching lines; `files_with_matches` returns only the paths that matched."}
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    /// Reading a subtree is a shared, read-only effect: independent searches in
    /// one round run concurrently instead of queueing behind a global lock.
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
        if input.query.is_empty() {
            return Err(ToolError::InvalidInput("query must not be empty".into()));
        }
        if input.max_results == 0 || input.max_results > MAX_MAX_RESULTS {
            return Err(ToolError::InvalidInput(format!(
                "max_results must be 1..={MAX_MAX_RESULTS}"
            )));
        }
        if input.context_lines > MAX_CONTEXT_LINES {
            return Err(ToolError::InvalidInput(format!(
                "context_lines must be 0..={MAX_CONTEXT_LINES}"
            )));
        }
        let plan = Plan {
            workspace: self.workspace.clone(),
            root: scope_root(
                &self.workspace,
                input.path.as_deref(),
                input.root.as_deref(),
            ),
            pattern: match input.mode {
                Mode::Literal => regex::escape(&input.query),
                Mode::Regex => input.query.clone(),
            },
            case_sensitive: input.case_sensitive,
            globs: Globs::compile(
                &string_list(input.include.as_ref()),
                &string_list(input.exclude.as_ref()),
            )?,
            context_lines: input.context_lines,
            max_results: input.max_results,
            output: input.output_mode,
        };
        let root = display_scope(&plan.workspace, &plan.root);
        let output = plan.output;
        let max_results = plan.max_results;
        let collected = tokio::task::spawn_blocking(move || run(&plan))
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))??;
        Ok(render(&root, output, max_results, collected))
    }
}

fn render(root: &str, output: OutputMode, max_results: usize, mut collected: Collected) -> String {
    let mut value = json!({
        "root": root,
        "count": 0,
        "truncated": collected.truncated,
        "skipped": collected.skipped,
        "files_scanned": collected.scanned,
    });
    match output {
        OutputMode::FilesWithMatches => {
            collected.files.sort_unstable();
            collected.files.truncate(max_results);
            value["count"] = json!(collected.files.len());
            value["files"] = json!(collected.files);
        }
        OutputMode::Content => {
            collected
                .hits
                .sort_by(|left, right| (&left.path, left.line).cmp(&(&right.path, right.line)));
            collected.hits.truncate(max_results);
            value["count"] = json!(collected.hits.len());
            value["matches"] =
                Value::Array(collected.hits.iter().map(Hit::to_json).collect::<Vec<_>>());
        }
    }
    value.to_string()
}

/// One bounded, ignore-aware scan. Returns an empty result — never an error —
/// when nothing matches.
fn run(plan: &Plan) -> Result<Collected, ToolError> {
    let regex = regex::RegexBuilder::new(&plan.pattern)
        .case_insensitive(!plan.case_sensitive)
        .build()
        .map_err(|error| {
            ToolError::InvalidInput(format!("invalid regex {:?}: {error}", plan.pattern))
        })?;
    let state = Mutex::new(Collected::default());
    let results = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    if std::fs::symlink_metadata(&plan.root).is_ok_and(|metadata| metadata.is_file()) {
        let relative = result_path(&plan.workspace, &plan.root, &plan.root);
        scan_file(
            plan, &regex, &state, &results, &stopped, &plan.root, relative,
        );
        return Ok(state
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner));
    }
    let mut builder = walker(&plan.root, None);
    builder.max_filesize(Some(MAX_FILE_BYTES));
    builder
        .build_parallel()
        .run(|| Box::new(|entry| visit(plan, &regex, &state, &results, &stopped, entry)));
    Ok(state
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn visit(
    plan: &Plan,
    regex: &Regex,
    state: &Mutex<Collected>,
    results: &AtomicUsize,
    stopped: &AtomicBool,
    entry: Result<DirEntry, ignore::Error>,
) -> WalkState {
    if stopped.load(Ordering::Relaxed) {
        return WalkState::Quit;
    }
    let Ok(entry) = entry else {
        count_skipped(state);
        return WalkState::Continue;
    };
    let Some(kind) = entry.file_type() else {
        count_skipped(state);
        return WalkState::Continue;
    };
    // Symlinks could point outside the bound root; directories carry no content.
    if kind.is_dir() || !kind.is_file() || kind.is_symlink() {
        return WalkState::Continue;
    }
    let name = entry.file_name().to_string_lossy();
    let relative = result_path(&plan.workspace, &plan.root, entry.path());
    if !plan.globs.allows(&relative, &name) {
        return WalkState::Continue;
    }
    scan_file(plan, regex, state, results, stopped, entry.path(), relative);
    WalkState::Continue
}

fn scan_file(
    plan: &Plan,
    regex: &Regex,
    state: &Mutex<Collected>,
    results: &AtomicUsize,
    stopped: &AtomicBool,
    path: &std::path::Path,
    relative: String,
) {
    let Ok(bytes) = std::fs::read(path) else {
        count_skipped(state);
        return;
    };
    if looks_binary(&bytes) {
        count_skipped(state);
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let mut local = Vec::new();
    let mut matched = 0_usize;
    for (index, line) in lines.iter().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        matched += 1;
        if plan.output == OutputMode::FilesWithMatches {
            break;
        }
        local.push(Hit {
            path: relative.clone(),
            line: index + 1,
            text: clip(line, MAX_LINE_CHARS),
            before: context(&lines, index, plan.context_lines, true),
            after: context(&lines, index, plan.context_lines, false),
        });
    }
    let mut collected = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    collected.scanned += 1;
    if matched == 0 {
        return;
    }
    let full = match plan.output {
        OutputMode::FilesWithMatches => {
            collected.files.push(relative);
            results.fetch_add(1, Ordering::Relaxed) + 1 >= plan.max_results
        }
        OutputMode::Content => {
            let mut full = false;
            for hit in local {
                collected.hits.push(hit);
                if results.fetch_add(1, Ordering::Relaxed) + 1 >= plan.max_results {
                    full = true;
                    break;
                }
            }
            full
        }
    };
    if full {
        collected.truncated = true;
        drop(collected);
        stopped.store(true, Ordering::Relaxed);
    }
}

fn count_skipped(state: &Mutex<Collected>) {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .skipped += 1;
}

/// `context_lines` neighbours on one side of `index`, clipped for output.
fn context(lines: &[&str], index: usize, context_lines: usize, before: bool) -> Vec<String> {
    if context_lines == 0 {
        return Vec::new();
    }
    if before {
        let start = index.saturating_sub(context_lines);
        lines[start..index]
            .iter()
            .map(|line| clip(line, MAX_CONTEXT_CHARS))
            .collect()
    } else {
        let end = (index + 1 + context_lines).min(lines.len());
        lines[index + 1..end]
            .iter()
            .map(|line| clip(line, MAX_CONTEXT_CHARS))
            .collect()
    }
}

fn clip(line: &str, limit: usize) -> String {
    line.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("ax-search-{}", uuid::Uuid::new_v4()));
        for dir in [
            "src",
            "target/debug",
            "node_modules/pkg",
            ".venv/lib",
            ".git/objects",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(
            root.join("src/lib.rs"),
            "fn alpha() {}\nlet needle = 1;\n中文 needle\n",
        )
        .unwrap();
        std::fs::write(root.join("src/other.rs"), "pub struct Needle;\n").unwrap();
        std::fs::write(root.join("target/debug/build.rs"), "needle").unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "needle").unwrap();
        std::fs::write(root.join(".venv/lib/site.rs"), "needle").unwrap();
        std::fs::write(root.join(".git/objects/blob"), "needle").unwrap();
        root
    }

    /// The tool is bound to `root`; a call then searches the bound workspace.
    async fn run_search(root: &std::path::Path, mut input: Value) -> Value {
        if input.get("path").is_none() {
            input["path"] = json!(".");
        }
        let output = SearchTool::new(root.to_path_buf())
            .execute(input)
            .await
            .unwrap();
        serde_json::from_str(&output).unwrap()
    }

    #[tokio::test]
    async fn literal_search_returns_paths_lines_and_skips_generated_trees() {
        let root = fixture();
        let result = run_search(&root, json!({"query":"needle"})).await;
        let paths: Vec<&str> = result["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["src/lib.rs", "src/lib.rs"]);
        assert_eq!(result["matches"][0]["line"], 2);
        assert_eq!(result["matches"][1]["line"], 3);
        assert_eq!(result["matches"][1]["text"], "中文 needle");
        assert_eq!(result["truncated"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn case_sensitive_is_default_and_can_be_disabled() {
        let root = fixture();
        let exact = run_search(&root, json!({"query":"Needle"})).await;
        assert_eq!(exact["count"], 1);
        let insensitive = run_search(&root, json!({"query":"Needle","case_sensitive":false})).await;
        assert_eq!(insensitive["count"], 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn regex_symbol_search_returns_context_lines() {
        let root = fixture();
        let result = run_search(
            &root,
            json!({"query":"fn\\s+alpha|struct\\s+Needle","mode":"regex","context_lines":1}),
        )
        .await;
        let hits = result["matches"].as_array().unwrap();
        assert_eq!(hits.len(), 2);
        let alpha = hits
            .iter()
            .find(|hit| hit["text"] == "fn alpha() {}")
            .unwrap();
        assert_eq!(alpha["after"][0], "let needle = 1;");
        assert!(alpha.get("before").is_none());
        let symbol = hits
            .iter()
            .find(|hit| hit["text"] == "pub struct Needle;")
            .unwrap();
        assert_eq!(symbol["path"], "src/other.rs");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn include_and_exclude_globs_narrow_the_scan() {
        let root = fixture();
        let result = run_search(
            &root,
            json!({"query":"needle","include":"*.rs","exclude":"src/lib.rs","case_sensitive":false}),
        )
        .await;
        let paths: Vec<&str> = result["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| hit["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, vec!["src/other.rs"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn files_with_matches_reports_each_file_once() {
        let root = fixture();
        let result = run_search(
            &root,
            json!({"query":"needle","case_sensitive":false,"output_mode":"files_with_matches"}),
        )
        .await;
        assert_eq!(result["files"], json!(["src/lib.rs", "src/other.rs"]));
        assert_eq!(result["count"], 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn no_match_is_a_successful_empty_result() {
        let root = fixture();
        let result = run_search(&root, json!({"query":"unlikely-ax-search-match-7b63"})).await;
        assert_eq!(result["matches"], json!([]));
        assert_eq!(result["count"], 0);
        assert_eq!(result["truncated"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn result_limit_truncates_without_failing() {
        let root = fixture();
        let result = run_search(&root, json!({"query":"needle","max_results":1})).await;
        assert_eq!(result["count"], 1);
        assert_eq!(result["truncated"], true);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_declares_read_only_resources() {
        let root = std::env::temp_dir().join("ax-search-resource");
        let access = SearchTool::new(root.clone()).resources(&json!({"query":"x"}));
        assert_eq!(access.len(), 1);
        assert!(!access[0].write);
        assert_eq!(access[0].resource, Resource::path(&root));
        // A nested scope is declared as itself, never as the whole workspace.
        let nested = SearchTool::new(root.clone()).resources(&json!({"query":"x","path":"src"}));
        assert_eq!(nested[0].resource, Resource::path(root.join("src")));
    }

    #[tokio::test]
    async fn results_are_reported_relative_to_the_bound_workspace() {
        let root = fixture();
        let result = run_search(&root, json!({"query":"needle","path":"src"})).await;
        assert_eq!(result["root"], "src");
        assert_eq!(result["matches"][0]["path"], "src/lib.rs");
        std::fs::remove_dir_all(root).unwrap();
    }
}
