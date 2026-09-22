use std::{collections::BTreeMap, process::Stdio};

use async_trait::async_trait;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use super::Transport;
use crate::McpError;

pub(crate) struct StdioTransport {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl StdioTransport {
    pub(crate) fn connect(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<&str>,
    ) -> Result<Self, McpError> {
        if command.trim().is_empty() {
            return Err(McpError::InvalidTransport(
                "stdio command must not be empty".to_owned(),
            ));
        }
        let mut process = Command::new(command);
        process
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let mut child = process
            .spawn()
            .map_err(|error| McpError::Transport(error.to_string()))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("stdio server stdin was not piped".to_owned()))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("stdio server stdout was not piped".to_owned()))?;
        Ok(Self {
            _child: child,
            input,
            output: BufReader::new(output),
        })
    }

    async fn send(&mut self, payload: &Value) -> Result<(), McpError> {
        let mut encoded = serde_json::to_vec(payload)
            .map_err(|error| McpError::InvalidResponse(error.to_string()))?;
        encoded.push(b'\n');
        self.input
            .write_all(&encoded)
            .await
            .map_err(|error| McpError::Transport(error.to_string()))?;
        self.input
            .flush()
            .await
            .map_err(|error| McpError::Transport(error.to_string()))
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn request(
        &mut self,
        payload: &Value,
        _method: &str,
        _name: Option<&str>,
    ) -> Result<Value, McpError> {
        self.send(payload).await?;
        let expected_id = payload.get("id").cloned();
        loop {
            let mut line = String::new();
            let count = self
                .output
                .read_line(&mut line)
                .await
                .map_err(|error| McpError::Transport(error.to_string()))?;
            if count == 0 {
                return Err(McpError::Transport(
                    "stdio server closed its output".to_owned(),
                ));
            }
            let response = serde_json::from_str::<Value>(&line)
                .map_err(|error| McpError::InvalidResponse(error.to_string()))?;
            if response.get("id") == expected_id.as_ref() {
                return Ok(response);
            }
        }
    }

    async fn notify(&mut self, payload: &Value, _method: &str) -> Result<(), McpError> {
        self.send(payload).await
    }
}
