//! Disposable Unix driver. Rust retains configuration and independent app policy.
use crate::{HostPermissionStore, HostSurface, HostTarget, PermissionDecision, ToolError};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio};
use tokio::io::AsyncWriteExt;

/// A private process group contains only AX's helper and its Python driver.
/// Killing the group on cancellation also stops descendants of the helper.
pub(super) struct WorkerGroup(pub(super) Option<u32>);
impl Drop for WorkerGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            // Fixed system executable and a kernel-assigned PID, never model input.
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

async fn exchange(request: &Value) -> Result<Value, ToolError> {
    let directory =
        std::env::temp_dir().join(format!("ax-desktop-worker-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory)?;
    let _cleanup = DriverDirectory(directory.clone());
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let result = async {
        for (name, contents) in [
            ("worker.py", include_str!("desktop_unix/worker.py")),
            ("linux.py", include_str!("desktop_unix/linux.py")),
            ("macos.py", include_str!("desktop_unix/macos.py")),
        ] {
            std::fs::write(directory.join(name), contents)?;
        }
        let python = std::env::var_os("AX_DESKTOP_PYTHON").unwrap_or_else(|| {
            let venv = crate::host_home().join("desktop/venv/bin/python");
            if venv.is_file() {
                venv.into_os_string()
            } else {
                "python3".into()
            }
        });
        let mut child = tokio::process::Command::new(python)
            .arg("-I").arg(directory.join("worker.py"))
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .kill_on_drop(true).spawn().map_err(|error| ToolError::Execution(format!("Cannot start Unix desktop Python runtime: {error}. Install the dependencies documented in computer-use.md or set AX_DESKTOP_PYTHON.")))?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::Execution("Missing desktop driver input".into()))?;
        input
            .write_all(
                &serde_json::to_vec(request)
                    .map_err(|error| ToolError::InvalidInput(error.to_string()))?,
            )
            .await?;
        input.shutdown().await?;
        drop(input); // JSON reader waits for EOF; shutdown alone does not close the pipe.
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(ToolError::Execution(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
            ToolError::Execution(format!("Invalid Unix desktop response: {error}"))
        })?;
        if let Some(error) = value["error"].as_str() {
            return Err(ToolError::Execution(error.to_owned()));
        }
        Ok(value)
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), result)
        .await
        .unwrap_or_else(|_| Err(ToolError::Execution("Unix desktop driver timed out".into())))
}

struct DriverDirectory(std::path::PathBuf);
impl Drop for DriverDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn target(value: &Value) -> Result<HostTarget, ToolError> {
    let path = value["app_id"]
        .as_str()
        .ok_or_else(|| ToolError::Execution("No verified application identity".into()))?;
    let id = crate::app_id(Path::new(path))?;
    Ok(HostTarget {
        surface: HostSurface::Computer,
        id,
        label: value["app_name"]
            .as_str()
            .unwrap_or("application")
            .to_owned(),
    })
}

pub(super) async fn run(request: &Value) -> Result<Value, ToolError> {
    tokio::time::timeout(std::time::Duration::from_secs(20), run_inner(request))
        .await
        .unwrap_or_else(|_| {
            Err(ToolError::Execution(
                "Unix desktop operation timed out".into(),
            ))
        })
}

async fn run_inner(request: &Value) -> Result<Value, ToolError> {
    let policy = HostPermissionStore::new(crate::host_home());
    if request["input"]["action"] == "list_windows" {
        let mut result = exchange(request).await?;
        let mut windows = Vec::new();
        for mut entry in result["windows"].as_array().cloned().unwrap_or_default() {
            let app = target(&entry)?;
            let access = policy.decision(&app)?;
            if access == PermissionDecision::Deny {
                continue;
            }
            if access != PermissionDecision::Allow {
                entry["title"] = json!("");
            }
            entry["app_id"] = json!(app.id);
            entry["access"] = serde_json::to_value(access)
                .map_err(|error| ToolError::Execution(error.to_string()))?;
            windows.push(entry);
        }
        result["windows"] = json!(windows);
        return Ok(result);
    }
    let mut resolve = request.clone();
    resolve["resolve_only"] = json!(true);
    let identity = exchange(&resolve).await?;
    let app = target(&identity)?;
    if request["resolve_only"] == true {
        return Ok(json!({"app_id":app.id,"app_name":app.label}));
    }
    if policy.decision(&app)? == PermissionDecision::Deny
        || request["authorized_app"].as_str() != Some(app.id.as_str())
    {
        return Err(ToolError::PermissionDenied(
            "Application access is not authorized; request app access first".into(),
        ));
    }
    // The driver resolves the window/application again before every effect.
    let mut action = request.clone();
    action["policy_path"] = json!(crate::host_home().join("host-permissions.json"));
    exchange(&action).await
}
