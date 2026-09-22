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
}
const fn default_limit() -> usize {
    100
}
pub struct SearchTool;
#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &'static str {
        "search"
    }
    fn description(&self) -> &'static str {
        "Search literal text in UTF-8 files. Returns paths, 1-based line numbers and snippets. Skips symlinks, .git, target and node_modules. Limits traversal and results; reports truncated scans."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"},"query":{"type":"string"},"max_results":{"type":"integer","minimum":1,"maximum":200}},"required":["path","query"],"additionalProperties":false})
    }
    fn capability(&self, _input: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        if input.query.is_empty() || !(1..=200).contains(&input.max_results) {
            return Err(ToolError::InvalidInput(
                "empty query or invalid result limit".into(),
            ));
        }
        let mut pending = vec![(input.path, 0)];
        let mut results = Vec::new();
        let mut visited = 0;
        let mut skipped = 0;
        while let Some((path, depth)) = pending.pop() {
            if visited >= 10_000 || results.len() >= input.max_results {
                pending.push((path, depth));
                break;
            }
            visited += 1;
            let metadata = match tokio::fs::symlink_metadata(&path).await {
                Ok(m) => m,
                Err(error) => {
                    if visited == 1 {
                        return Err(ToolError::Execution(error.to_string()));
                    }
                    skipped += 1;
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                skipped += 1;
                continue;
            }
            if metadata.is_dir() {
                if depth >= 32 {
                    skipped += 1;
                    continue;
                }
                let mut entries = tokio::fs::read_dir(&path)
                    .await
                    .map_err(|e| ToolError::Execution(e.to_string()))?;
                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|e| ToolError::Execution(e.to_string()))?
                {
                    if !matches!(
                        entry.file_name().to_str(),
                        Some(".git" | "target" | "node_modules")
                    ) {
                        pending.push((entry.path(), depth + 1));
                    }
                    if pending.len() >= 10_000 {
                        skipped += 1;
                        break;
                    }
                }
            } else if metadata.is_file() && metadata.len() <= 2_000_000 {
                let Ok(text) = tokio::fs::read_to_string(&path).await else {
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
}
