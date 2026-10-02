//! Bounded literal file search with structured path/line results.
use crate::{Capability, SafetyLevel, Tool, ToolError};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: PathBuf,
    query: String,
    #[serde(default = "default_limit")]
    max_results: usize,
    #[serde(default, rename = "fallback_reason")]
    _fallback_reason: Option<String>,
}
const fn default_limit() -> usize {
    100
}
pub struct SearchTool;
#[async_trait]
impl Tool for SearchTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::WorkspaceWorker
    }
    fn fork_for_run(&self, context: &crate::RunContext) -> Option<std::sync::Arc<dyn Tool>> {
        Some(std::sync::Arc::new(crate::WorkspaceTool::new(
            std::sync::Arc::new(SearchTool),
            context.cwd.clone(),
        )))
    }
    fn recursive_search(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "search"
    }
    fn description(&self) -> &'static str {
        "Search exact literal text in a targeted file/subtree before reading code. Batch independent searches/reads in the SAME model response; the DAG executes them concurrently. Returns paths, 1-based lines and snippets; read small hit ranges next. Do not enumerate broad directories first."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"query":{"type":"string"},"max_results":{"type":"integer","minimum":1,"maximum":200},"fallback_reason":{"type":"string","description":"Optional orchestration note explaining a broader search; does not gate execution"}},"required":["path","query"],"additionalProperties":false})
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn resources(&self, input: &Value) -> Vec<crate::ResourceAccess> {
        input["path"].as_str().map_or_else(
            || vec![crate::ResourceAccess::exclusive()],
            |path| vec![crate::ResourceAccess::read(crate::Resource::path(path))],
        )
    }
    #[allow(clippy::too_many_lines)] // One bounded traversal with partial-result accounting.
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if input.query.is_empty() || !(1..=200).contains(&input.max_results) {
            return Err(ToolError::InvalidInput(
                "empty query or invalid result limit".into(),
            ));
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut pending = vec![(input.path, 0)];
        let mut results = Vec::new();
        let mut visited = 0;
        let mut skipped = 0;
        while let Some((path, depth)) = pending.pop() {
            if visited >= 2_000
                || tokio::time::Instant::now() >= deadline
                || results.len() >= input.max_results
            {
                pending.push((path, depth));
                break;
            }
            visited += 1;
            let metadata =
                match tokio::time::timeout_at(deadline, tokio::fs::symlink_metadata(&path)).await {
                    Err(_) => {
                        pending.push((path, depth));
                        break;
                    }
                    Ok(result) => match result {
                        Ok(m) => m,
                        Err(error) => {
                            if visited == 1 {
                                return Err(crate::path_error(&path, &error));
                            }
                            skipped += 1;
                            continue;
                        }
                    },
                };
            if metadata.file_type().is_symlink() {
                skipped += 1;
                continue;
            }
            if metadata.is_dir() {
                if depth >= 8 {
                    skipped += 1;
                    continue;
                }
                let mut entries = tokio::time::timeout_at(deadline, tokio::fs::read_dir(&path))
                    .await
                    .map_err(|_| ToolError::Execution("search time limit reached".into()))?
                    .map_err(|e| ToolError::Execution(e.to_string()))?;
                while let Some(entry) = tokio::time::timeout_at(deadline, entries.next_entry())
                    .await
                    .map_err(|_| ToolError::Execution("search time limit reached".into()))?
                    .map_err(|e| ToolError::Execution(e.to_string()))?
                {
                    if !matches!(
                        entry.file_name().to_str(),
                        Some(
                            ".git"
                                | "target"
                                | "build"
                                | "dist"
                                | "node_modules"
                                | "child-runs"
                                | ".cache"
                                | "cache"
                                | "__pycache__"
                                | ".venv"
                                | "venv"
                                | ".ax"
                                | "coverage"
                                | "vendor"
                        )
                    ) {
                        pending.push((entry.path(), depth + 1));
                    }
                    if pending.len() + visited >= 2_000 {
                        skipped += 1;
                        break;
                    }
                }
            } else if metadata.is_file() && metadata.len() <= 1_000_000 {
                let Ok(Ok(text)) =
                    tokio::time::timeout_at(deadline, tokio::fs::read_to_string(&path)).await
                else {
                    skipped += 1;
                    continue;
                };
                if text.contains('\0') {
                    skipped += 1;
                    continue;
                }
                for (index, line) in text.lines().enumerate() {
                    if line.contains(&input.query) {
                        results.push(json!({"path":path,"line":index+1,"text":line.chars().take(500).collect::<String>()}));
                        if results.len() >= input.max_results {
                            skipped += 1;
                            break;
                        }
                    }
                }
            } else {
                skipped += 1;
            }
        }
        Ok(json!({"matches":results,"truncated":!pending.is_empty() || skipped>0,"skipped":skipped,"visited":visited}).to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn search_returns_line_numbers_and_obeys_limit() {
        let path = std::env::temp_dir().join(format!("ax-search-{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&path, "one\n中文 needle\nneedle\n")
            .await
            .unwrap();
        let output = SearchTool
            .execute(json!({"path":path,"query":"needle","max_results":1}))
            .await
            .unwrap();
        let result: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["matches"][0]["line"], 2);
        assert_eq!(result["matches"].as_array().unwrap().len(), 1);
        assert_eq!(result["truncated"], true);
        tokio::fs::remove_file(path).await.unwrap();
    }
    #[tokio::test]
    async fn workspace_search_needs_no_semantic_fallback_permission() {
        let output = SearchTool
            .execute(json!({"path":".","query":"unlikely-ax-search-match-7b63"}))
            .await
            .unwrap();
        assert!(serde_json::from_str::<Value>(&output).unwrap()["matches"].is_array());
    }
    #[tokio::test]
    async fn excludes_generated_directories_large_files_and_deep_subtrees() {
        let root = std::env::temp_dir().join(format!("ax-search-bounds-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("wanted.txt"), "needle")
            .await
            .unwrap();
        for name in [
            ".git",
            "target",
            "build",
            "node_modules",
            "child-runs",
            ".cache",
            "__pycache__",
            ".venv",
        ] {
            tokio::fs::create_dir(root.join(name)).await.unwrap();
            tokio::fs::write(root.join(name).join("hidden.txt"), "needle")
                .await
                .unwrap();
        }
        tokio::fs::write(root.join("large.txt"), "needle".repeat(200_000))
            .await
            .unwrap();
        let deep = (0..10).fold(root.clone(), |path, n| path.join(n.to_string()));
        tokio::fs::create_dir_all(&deep).await.unwrap();
        tokio::fs::write(deep.join("hidden.txt"), "needle")
            .await
            .unwrap();
        let output = SearchTool
            .execute(json!({"path":root,"query":"needle"}))
            .await
            .unwrap();
        let result: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["matches"].as_array().unwrap().len(), 1);
        assert!(
            result["matches"][0]["path"]
                .as_str()
                .unwrap()
                .ends_with("wanted.txt")
        );
        assert_eq!(result["truncated"], true);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
    #[tokio::test]
    async fn traversal_has_a_global_entry_ceiling() {
        let root = std::env::temp_dir().join(format!("ax-search-count-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        for n in 0..2_010 {
            tokio::fs::write(root.join(n.to_string()), "text")
                .await
                .unwrap();
        }
        let output = SearchTool
            .execute(json!({"path":root,"query":"absent"}))
            .await
            .unwrap();
        let result: Value = serde_json::from_str(&output).unwrap();
        assert!(result["visited"].as_u64().unwrap() <= 2_000);
        assert_eq!(result["truncated"], true);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
