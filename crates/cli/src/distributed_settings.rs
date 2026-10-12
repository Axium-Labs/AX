//! Remote administration invokes the same validated native settings commands as the desktop.
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};

fn clean(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(k, _)| {
                    ![
                        "userConfig",
                        "env",
                        "headers",
                        "token",
                        "credential",
                        "auth",
                        "api_key",
                        "password",
                    ]
                    .contains(&k.as_str())
                })
                .map(|(k, v)| (k, clean(v)))
                .collect(),
        ),
        Value::Array(items) => json!(items.into_iter().map(clean).collect::<Vec<_>>()),
        Value::String(text) => {
            let filtered = crate::long_project::safe(&text);
            // Preserve nonsecret formatting so native expected-content checks remain exact.
            if filtered.trim_end_matches('\n') == text.replace("\r\n", "\n").trim_end_matches('\n')
            {
                json!(text)
            } else {
                json!(filtered)
            }
        }
        other => other,
    }
}
// Temporary native CLI inputs stay on the target AX host and are removed on every outcome.
struct NativeInputs(PathBuf);
impl Drop for NativeInputs {
    fn drop(&mut self) {
        for name in ["instructions.md", "expected.md"] {
            let _ = std::fs::remove_file(self.0.join(name));
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}
#[allow(clippy::too_many_lines)]
async fn execute(
    home: &Path,
    projects: &BTreeMap<String, PathBuf>,
    command: &Value,
    busy: bool,
) -> Result<Value> {
    let action = command["action"].as_str().unwrap_or_default();
    let read = action.ends_with("read") || action.ends_with("list");
    ensure!(
        read || !busy,
        "AX has active executions; pause or finish its tasks before changing instance settings"
    );
    let scope = command["scope"].as_str().unwrap_or("global");
    ensure!(
        ["global", "project"].contains(&scope),
        "invalid native scope"
    );
    let cwd = if scope == "project" {
        projects
            .get(command["project_id"].as_str().unwrap_or_default())
            .ok_or_else(|| anyhow!("project mapping not approved"))?
            .as_path()
    } else {
        home
    };
    let input = &command["input"];
    let mut native = Command::new(std::env::current_exe()?);
    native
        .env("AX_HOME", home)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    native.creation_flags(0x0800_0000);
    let mut stdin = None;
    let mut _inputs = None;
    match action {
        "settings.read" | "settings.write" => {
            native.args(["settings", "--scope", scope]);
            if action.ends_with("write") {
                let value = input
                    .as_object()
                    .ok_or_else(|| anyhow!("settings must be an object"))?;
                ensure!(
                    value
                        .keys()
                        .all(|k| ["max_depth", "max_concurrent"].contains(&k.as_str())),
                    "unsupported settings field"
                );
                for (key, value) in value {
                    let n = value
                        .as_u64()
                        .ok_or_else(|| anyhow!("invalid integer setting"))?;
                    native
                        .arg(format!("--{}", key.replace('_', "-")))
                        .arg(n.to_string());
                }
            }
        }
        "goal.read" | "goal.write" => {
            native.args(["goal-settings", "--scope", scope]);
            if action.ends_with("write") {
                native.arg("--json-stdin").stdin(Stdio::piped());
                stdin = Some(input.to_string());
            }
        }
        "personalize.read" | "personalize.write" => {
            ensure!(
                scope == "global",
                "personalization is an AX Instance global setting"
            );
            native.arg("personalize");
            if action.ends_with("write") {
                let fields = input
                    .as_object()
                    .ok_or_else(|| anyhow!("personalization must be an object"))?;
                ensure!(
                    fields.keys().all(|k| [
                        "memory",
                        "tool_memory",
                        "writing",
                        "instructions",
                        "expected_instructions",
                        "writing_folder",
                        "clear_writing_folder"
                    ]
                    .contains(&k.as_str())),
                    "unsupported personalization field"
                );
                if let Some(text) = input["instructions"].as_str() {
                    let expected = input["expected_instructions"]
                        .as_str()
                        .ok_or_else(|| anyhow!("read instructions before editing"))?;
                    ensure!(
                        crate::long_project::safe(text).trim_end_matches('\n')
                            == text.replace("\r\n", "\n").trim_end_matches('\n'),
                        "instructions may not contain credentials"
                    );
                    let dir = home.join(format!(".remote-inputs-{}", uuid::Uuid::new_v4()));
                    std::fs::create_dir(&dir)?;
                    let inputs = NativeInputs(dir);
                    std::fs::write(inputs.0.join("instructions.md"), text)?;
                    std::fs::write(inputs.0.join("expected.md"), expected)?;
                    native
                        .arg("--instructions-file")
                        .arg(inputs.0.join("instructions.md"))
                        .arg("--expected-instructions-file")
                        .arg(inputs.0.join("expected.md"));
                    _inputs = Some(inputs);
                }
                if let Some(folder) = input["writing_folder"].as_str() {
                    let project = projects
                        .get(command["project_id"].as_str().unwrap_or_default())
                        .ok_or_else(|| anyhow!("choose an approved project for writing samples"))?;
                    let folder = project.join(folder).canonicalize()?;
                    ensure!(
                        folder.is_dir() && folder.starts_with(project.canonicalize()?),
                        "writing folder escapes approved project"
                    );
                    native.arg("--writing-folder").arg(folder);
                }
                if input["clear_writing_folder"] == true {
                    native.arg("--clear-writing-folder");
                }
                for (key, value) in fields
                    .iter()
                    .filter(|(k, _)| ["memory", "tool_memory", "writing"].contains(&k.as_str()))
                {
                    let enabled = value
                        .as_bool()
                        .ok_or_else(|| anyhow!("invalid personalization switch"))?;
                    native
                        .arg(format!("--{}", key.replace('_', "-")))
                        .arg(enabled.to_string());
                }
            }
        }
        "plugins.list" | "plugins.enable" | "plugins.disable" | "plugins.import" => {
            let kind = input["kind"].as_str().unwrap_or("skills");
            ensure!(
                ["skills", "mcp", "agents", "mods"].contains(&kind),
                "unsupported plugin kind"
            );
            let operation = action.strip_prefix("plugins.").unwrap();
            native.args([
                "capabilities",
                kind,
                if operation == "import" {
                    "add"
                } else {
                    operation
                },
                "--scope",
                scope,
            ]);
            if ["enable", "disable", "import"].contains(&operation) {
                let name = input["name"]
                    .as_str()
                    .ok_or_else(|| anyhow!("plugin name required"))?;
                crate::capabilities::validate_name(name)?;
                native.arg(name);
            }
            if operation == "import" {
                let project = projects
                    .get(command["project_id"].as_str().unwrap_or_default())
                    .ok_or_else(|| anyhow!("choose an approved project for plugin source"))?;
                let path = project
                    .join(
                        input["source"]
                            .as_str()
                            .ok_or_else(|| anyhow!("project-relative source required"))?,
                    )
                    .canonicalize()?;
                ensure!(
                    path.starts_with(project.canonicalize()?) && path != project.canonicalize()?,
                    "plugin source escapes approved project"
                );
                native.arg("--source").arg(path);
            }
        }
        _ => return Err(anyhow!("unsupported native action")),
    }
    let mut child = native.spawn()?;
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("native stdin absent"))?
            .write_all(text.as_bytes())
            .await?;
    }
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await??;
    ensure!(
        output.status.success(),
        "native settings: {}",
        crate::long_project::safe(&String::from_utf8_lossy(&output.stderr))
    );
    ensure!(
        output.stdout.len() <= 65536,
        "native configuration output too large"
    );
    Ok(clean(serde_json::from_slice(&output.stdout)?))
}
pub(crate) async fn apply(
    home: PathBuf,
    projects: BTreeMap<String, PathBuf>,
    commands: Vec<Value>,
    busy: bool,
) -> Vec<Value> {
    let mut results = vec![];
    for command in commands {
        let result = execute(&home, &projects, &command, busy).await;
        results.push(json!({"id":command["id"],"status":if result.is_ok(){"completed"}else{"failed"},"result":match result {Ok(value)=>value,Err(error)=>json!({"error":crate::long_project::safe(&error.to_string())})}}));
    }
    results
}

/// Carry approved project policy/definitions into an execution, never runtime stores.
/// The source project's Memory, Sessions, auth and long-project histories stay local.
pub(crate) fn snapshot_project(source: &Path, workspace: &Path) -> Result<()> {
    let from = source.join(".ax");
    if !from.exists() {
        return Ok(());
    }
    ensure!(
        !std::fs::symlink_metadata(&from)?.file_type().is_symlink(),
        "project settings reject symbolic links"
    );
    let to = workspace.join(".ax");
    std::fs::create_dir_all(&to)?;
    let document = scoped::read_document(&from.join("config.toml"))?;
    let mut approved = toml::map::Map::new();
    for key in ["subagent", "skills", "mcp", "agents", "mods"] {
        if let Some(value) = document.get(key) {
            approved.insert(key.into(), value.clone());
        }
    }
    if !approved.is_empty() {
        scoped::write_document(&to.join("config.toml"), &toml::Value::Table(approved))?;
    }
    for name in ["goal.json", "mcp.toml"] {
        let path = from.join(name);
        if path.exists() {
            ensure!(
                std::fs::symlink_metadata(&path)?.file_type().is_file(),
                "project policy must be a regular file"
            );
            std::fs::copy(path, to.join(name))?;
        }
    }
    for name in ["skills", "agents", "mods"] {
        let path = from.join(name);
        if path.exists() {
            crate::distributed_worker::copy_snapshot(&path, &to.join(name))?;
        }
    }
    Ok(())
}
