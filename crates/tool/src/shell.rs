use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{SafetyLevel, Tool, ToolError};

pub struct ShellTool;

#[derive(Deserialize)]
struct ShellInput {
    command: String,
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Run a shell command in the current working directory. Shell execution always requires approval."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Command to execute" }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::RequiresApproval
    }

    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let input: ShellInput = serde_json::from_value(input)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        if input.command.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "command must not be empty".to_owned(),
            ));
        }

        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("powershell");
            command.args(["-NoLogo", "-NoProfile", "-Command", &input.command]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-lc", &input.command]);
            command
        };

        let output = command
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        Ok(format!(
            "exit_code: {}\nstdout:\n{}\nstderr:\n{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        ))
    }
}
