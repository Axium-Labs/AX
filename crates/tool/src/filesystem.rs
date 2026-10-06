use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{SafetyLevel, Tool, ToolError};

pub struct FilesystemTool;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum FilesystemInput {
    Read {
        path: String,
        start_line: Option<usize>,
        end_line: Option<usize>,
    },
    List {
        path: String,
    },
    Write {
        path: String,
        content: String,
    },
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for FilesystemTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::WorkspaceWorker
    }
    fn fork_for_run(&self, context: &crate::RunContext) -> Option<std::sync::Arc<dyn Tool>> {
        Some(std::sync::Arc::new(crate::WorkspaceTool::new(
            std::sync::Arc::new(FilesystemTool),
            context.cwd.clone(),
        )))
    }
    fn name(&self) -> &str {
        "filesystem"
    }

    fn description(&self) -> &str {
        "Read, list or write a known path. `read` accepts a 1-based start_line/end_line range when sufficient; omit the range for full file context. `list` shows exactly one directory level and is not a discovery tool: prefer find_files/glob for unknown locations and search for text or symbols. After discovery, prefer direct reads for context; targeted searches within known files remain useful. Batch independent reads when possible. Prefer patch for localized edits; write creates or replaces a whole file and is appropriate for requested full-file rewrites or generated artifacts. Inspect existing content before replacing it; writes require approval."
    }

    fn guidance(&self) -> Option<&'static str> {
        Some(
            "filesystem: read only the smallest sufficient 1-based line range; omit the range only \
             when the whole file is genuinely needed. `list` returns one directory level and is not \
             a discovery tool. Batch independent reads of already-known paths into one call rather \
             than alternating read -> model -> read.",
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": { "type": "string", "enum": ["read", "list", "write"], "description": "`read` a file (optionally a 1-based line range), `list` one directory level, or `write` create or replace an entire file (requires approval)." },
                "path": { "type": "string", "description": "Path relative to the workspace root." },
                "content": { "type": "string", "description": "Complete new file content for write; replaces existing content rather than appending." },
                "start_line": { "type":"integer", "minimum":1 },
                "end_line": { "type":"integer", "minimum":1 }
            },
            "required": ["operation", "path"],
            "additionalProperties": false
        })
    }

    fn capability(&self, input: &Value) -> crate::Capability {
        if input.get("operation").and_then(Value::as_str) == Some("write") {
            crate::Capability::FilesystemWrite
        } else {
            crate::Capability::FilesystemRead
        }
    }

    fn safety(&self, input: &Value) -> SafetyLevel {
        if input.get("operation").and_then(Value::as_str) == Some("write") {
            SafetyLevel::RequiresApproval
        } else {
            SafetyLevel::Safe
        }
    }

    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: FilesystemInput = serde_json::from_value(input)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        match input {
            FilesystemInput::Read {
                path,
                start_line,
                end_line,
            } => {
                let text = tokio::fs::read_to_string(&path)
                    .await
                    .map_err(|error| crate::path_error(std::path::Path::new(&path), &error))?;
                if start_line.is_none() && end_line.is_none() {
                    return Ok(text);
                }
                let start = start_line.unwrap_or(1);
                let end = end_line.unwrap_or(start.saturating_add(79));
                if start == 0 || end < start {
                    return Err(ToolError::InvalidInput("invalid line range".into()));
                }
                Ok(text
                    .lines()
                    .enumerate()
                    .skip(start - 1)
                    .take(end - start + 1)
                    .map(|(i, line)| format!("{}: {line}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            FilesystemInput::List { path } => {
                let mut entries = tokio::fs::read_dir(&path)
                    .await
                    .map_err(|error| crate::path_error(std::path::Path::new(&path), &error))?;
                let mut names = Vec::new();
                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|error| ToolError::Execution(error.to_string()))?
                {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
                names.sort_unstable();
                Ok(names.join("\n"))
            }
            FilesystemInput::Write { path, content } => {
                tokio::fs::write(path, content)
                    .await
                    .map_err(|error| ToolError::Execution(error.to_string()))?;
                Ok("write completed".to_owned())
            }
        }
    }

    fn resources(&self, input: &Value) -> Vec<crate::ResourceAccess> {
        let Some(path) = input["path"].as_str() else {
            return vec![crate::ResourceAccess::exclusive()];
        };
        let resource = crate::Resource::path(path);
        vec![if input["operation"] == "write" {
            crate::ResourceAccess::write(resource)
        } else {
            crate::ResourceAccess::read(resource)
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn ranged_read_and_missing_path_include_recovery_information() {
        let root = std::env::temp_dir().join(format!("ax-read-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&root).await.unwrap();
        let path = root.join("actual.rs");
        tokio::fs::write(&path, "one\ntwo\nthree\n").await.unwrap();
        let result = FilesystemTool
            .execute(json!({"operation":"read","path":path,"start_line":2,"end_line":2}))
            .await
            .unwrap();
        assert_eq!(result, "2: two");
        let error = FilesystemTool
            .execute(json!({"operation":"read","path":root.join("missing.rs")}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("candidate_paths"));
        assert!(error.to_string().contains("actual.rs"));
        tokio::fs::remove_file(path).await.unwrap();
        tokio::fs::remove_dir(root).await.unwrap();
    }
}
