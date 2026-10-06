//! Local AX drives configured SSH hosts; no agent or model runs remotely.
use crate::{
    Capability, ExecutionBoundary, PermissionProfile, Resource, ResourceAccess, RunContext,
    SafetyLevel, Tool, ToolError, ToolOutput,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{process::Stdio, sync::Arc};
use tokio::{io::AsyncWriteExt, process::Command};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SshHost {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}
impl SshHost {
    /// # Errors
    /// Rejects option-like hosts, malformed destinations, ports and key paths.
    pub fn validate(&self) -> Result<(), ToolError> {
        if self.id.is_empty()
            || self.host.is_empty()
            || self.host.starts_with('-')
            || !self
                .host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-@:[]".contains(&c))
            || self.host.matches('@').count() > 1
            || self
                .host
                .split('@')
                .any(|p| p.is_empty() || p.starts_with('-'))
            || self.port == Some(0)
            || self
                .identity_file
                .as_ref()
                .is_some_and(|p| p.contains(['\0', '\n', '\r']))
        {
            return Err(ToolError::InvalidInput(
                "invalid configured SSH host".into(),
            ));
        }
        Ok(())
    }
    /// The program and option names are fixed; user commands only reach remote stdin.
    /// # Errors
    /// Returns validation errors before starting any process.
    pub fn command(&self) -> Result<Command, ToolError> {
        self.validate()?;
        let mut command = Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "StrictHostKeyChecking=accept-new",
        ]);
        if let Some(port) = self.port {
            command.arg("-p").arg(port.to_string());
        }
        if let Some(path) = self.identity_file.as_deref().filter(|p| !p.is_empty()) {
            let path =
                if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
                    std::env::var_os("USERPROFILE")
                        .or_else(|| std::env::var_os("HOME"))
                        .map(std::path::PathBuf::from)
                        .unwrap_or_default()
                        .join(rest)
                } else {
                    std::path::PathBuf::from(path)
                };
            command.arg("-i").arg(path);
        }
        command
            .arg(&self.host)
            .arg("sh -s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        Ok(command)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SshContext {
    pub hosts: Vec<SshHost>,
    pub default_host: String,
    pub cwd: String,
}
#[derive(Clone)]
pub struct SshTool {
    context: SshContext,
}
#[must_use]
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
impl SshTool {
    /// # Errors
    /// Requires unique valid hosts and a valid selected host/directory.
    pub fn new(context: SshContext) -> Result<Self, ToolError> {
        for host in &context.hosts {
            host.validate()?;
        }
        if !context.hosts.iter().any(|h| h.id == context.default_host) || context.cwd.contains('\0')
        {
            return Err(ToolError::InvalidInput("invalid SSH context".into()));
        }
        let mut ids = std::collections::HashSet::new();
        if context.hosts.iter().any(|h| !ids.insert(&h.id)) {
            return Err(ToolError::InvalidInput("duplicate SSH host ID".into()));
        }
        Ok(Self { context })
    }
    /// # Errors
    /// Rejects malformed or invalid SSH context instead of falling back locally.
    pub fn from_env() -> Result<Option<Self>, ToolError> {
        let raw = if let Some(path) = std::env::var_os("AX_SSH_CONTEXT_FILE") {
            Some(std::fs::read_to_string(path)?)
        } else {
            std::env::var("AX_SSH_CONTEXT").ok()
        };
        raw.map(|raw| {
            let context =
                serde_json::from_str(&raw).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
            Self::new(context)
        })
        .transpose()
    }
    #[must_use]
    pub fn instructions(&self) -> String {
        format!(
            "SSH execution: You are AX running LOCALLY with local model credentials and local session history. Use the ssh tool to read/write files, run tests and execute shell commands on remote hosts. No AX is installed or started there. Default host ID: {}; default remote directory: {}. Call ssh action=list for all configured hosts; host_id selects another host. Independent hosts may be called in parallel. Remote paths must never be treated as local files.",
            self.context.default_host, self.context.cwd
        )
    }
    fn host(&self, input: &Value) -> Result<&SshHost, ToolError> {
        let id = match input.get("host_id") {
            Some(value) => value
                .as_str()
                .ok_or_else(|| ToolError::InvalidInput("host_id must be a string".into()))?,
            None => &self.context.default_host,
        };
        self.context
            .hosts
            .iter()
            .find(|h| h.id == id)
            .ok_or_else(|| ToolError::InvalidInput("unknown configured SSH host_id".into()))
    }
}
#[async_trait]
impl Tool for SshTool {
    fn name(&self) -> &'static str {
        "ssh"
    }
    fn description(&self) -> &'static str {
        "Run remote shell commands through local OpenSSH, or list configured hosts. AX and model inference stay local. Use commands such as cat, find, git, or scripts to work on remote files."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"action":{"type":"string","enum":["list","exec"]},"host_id":{"type":"string","description":"Configured ID from list; omitted uses selected host"},"command":{"type":"string","description":"Remote POSIX shell command"},"cwd":{"type":"string","description":"Remote working directory; selected host default applies only to that host"},"timeout_seconds":{"type":"integer","minimum":1}},"required":["action"],"additionalProperties":false})
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        if input["action"] == "list" {
            SafetyLevel::Safe
        } else {
            SafetyLevel::RequiresApproval
        }
    }
    fn capability(&self, input: &Value) -> Capability {
        if input["action"] == "list" {
            Capability::FilesystemRead
        } else {
            Capability::Shell
        }
    }
    fn independent_remote_execution(&self) -> bool {
        true
    }
    fn execution_boundary(&self) -> ExecutionBoundary {
        ExecutionBoundary::Remote
    }
    fn fork_for_run(&self, _: &RunContext) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(self.clone()))
    }
    fn resources(&self, input: &Value) -> Vec<ResourceAccess> {
        if input["action"] == "list" {
            vec![]
        } else {
            self.host(input).map_or_else(
                |_| vec![ResourceAccess::exclusive()],
                |host| {
                    vec![ResourceAccess::write(Resource::Named(format!(
                        "ssh:{}:{}",
                        host.host,
                        host.port.unwrap_or(22)
                    )))]
                },
            )
        }
    }
    async fn execute_output_constrained(
        &self,
        input: Value,
        profiles: &[PermissionProfile],
    ) -> Result<ToolOutput, ToolError> {
        if input["action"] != "list" && profiles.iter().any(PermissionProfile::has_network_rules) {
            return Err(ToolError::PermissionDenied(
                "SSH cannot enforce domain-level network rules".into(),
            ));
        }
        self.execute(input).await.map(ToolOutput::Text)
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        if input["action"] == "list" {
            return Ok(json!({"default_host":self.context.default_host,"cwd":self.context.cwd,"hosts":self.context.hosts.iter().map(|h|json!({"id":h.id,"name":h.name,"host":h.host,"port":h.port})).collect::<Vec<_>>()} ).to_string());
        }
        if input["action"] != "exec" {
            return Err(ToolError::InvalidInput(
                "action must be list or exec".into(),
            ));
        }
        let host = self.host(&input)?;
        let command = input["command"]
            .as_str()
            .filter(|s| !s.is_empty() && !s.contains('\0'))
            .ok_or_else(|| ToolError::InvalidInput("command required".into()))?;
        let cwd = input["cwd"]
            .as_str()
            .unwrap_or(if host.id == self.context.default_host {
                &self.context.cwd
            } else {
                "."
            });
        if cwd.contains('\0') {
            return Err(ToolError::InvalidInput("invalid remote cwd".into()));
        }
        let mut child = host.command()?.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::Execution("SSH stdin unavailable".into()))?;
        let script = format!("cd -- {} || exit\n{}\n", quote(cwd), command);
        let timeout =
            std::time::Duration::from_secs(input["timeout_seconds"].as_u64().unwrap_or(120).max(1));
        let output = tokio::time::timeout(timeout, async {
            stdin.write_all(script.as_bytes()).await?;
            drop(stdin);
            child.wait_with_output().await
        })
        .await
        .map_err(|_| ToolError::Execution("SSH command timed out".into()))??;
        Ok(json!({"host_id":host.id,"cwd":cwd,"exit_code":output.status.code(),"stdout":String::from_utf8_lossy(&output.stdout),"stderr":String::from_utf8_lossy(&output.stderr)}).to_string())
    }
}
