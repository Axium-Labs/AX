use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{SafetyLevel, Tool, ToolError};

pub struct FilesystemTool;

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum FilesystemInput {
    Read { path: String },
    List { path: String },
    Write { path: String, content: String },
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for FilesystemTool {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn description(&self) -> &str {
        "Read, list, or write filesystem paths. Writes require explicit approval."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": { "type": "string", "enum": ["read", "list", "write"] },
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["operation", "path"],
            "additionalProperties": false
        })
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
            FilesystemInput::Read { path } => tokio::fs::read_to_string(path)
                .await
                .map_err(|error| ToolError::Execution(error.to_string())),
            FilesystemInput::List { path } => {
                let mut entries = tokio::fs::read_dir(path)
                    .await
                    .map_err(|error| ToolError::Execution(error.to_string()))?;
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
}
