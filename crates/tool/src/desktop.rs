//! Opt-in Windows accessibility automation. The host supplies immutable settings.
use crate::{Capability, SafetyLevel, Tool, ToolError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_DESKTOP_NODES: usize = 10_000;
pub const MAX_SCREENSHOT_WIDTH: u32 = 4096;
pub const MIN_SCREENSHOT_WIDTH: u32 = 320;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopSettings {
    pub enabled: bool,
    pub include_screenshot: bool,
    pub max_nodes: usize,
    pub screenshot_width: u32,
}
impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            include_screenshot: true,
            max_nodes: 1200,
            screenshot_width: 1280,
        }
    }
}
impl DesktopSettings {
    /// Validate user edits; hand-edited files are also clamped at execution time.
    ///
    /// # Errors
    /// Returns an error for node counts or image sizes outside built-in bounds.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_DESKTOP_NODES).contains(&self.max_nodes) {
            return Err(format!(
                "Tree node limit must be between 1 and {MAX_DESKTOP_NODES}"
            ));
        }
        if !(MIN_SCREENSHOT_WIDTH..=MAX_SCREENSHOT_WIDTH).contains(&self.screenshot_width) {
            return Err(format!(
                "Screenshot width must be between {MIN_SCREENSHOT_WIDTH} and {MAX_SCREENSHOT_WIDTH}"
            ));
        }
        Ok(())
    }
}
pub struct DesktopTool {
    settings: DesktopSettings,
    host: crate::HostPermissionStore,
}
impl DesktopTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_settings(DesktopSettings::default())
    }
    #[must_use]
    pub fn with_settings(mut settings: DesktopSettings) -> Self {
        settings.max_nodes = settings.max_nodes.clamp(1, MAX_DESKTOP_NODES);
        settings.screenshot_width = settings
            .screenshot_width
            .clamp(MIN_SCREENSHOT_WIDTH, MAX_SCREENSHOT_WIDTH);
        Self {
            settings,
            host: crate::HostPermissionStore::new(crate::host_home()),
        }
    }
    #[must_use]
    pub fn with_session(mut self, session: &str) -> Self {
        self.host = self.host.with_session(session);
        self
    }
    fn request(&self, input: &Value) -> Result<Value, ToolError> {
        if !self.settings.enabled {
            return Err(ToolError::PermissionDenied("Computer Use is disabled. Enable it in AX Crew settings before starting a new turn.".into()));
        }
        let action = input["action"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("Missing desktop action".into()))?;
        if ![
            "list_windows",
            "read_window",
            "screenshot_window",
            "click_element",
            "type_text",
            "press_key",
            "move_mouse",
            "mouse_click",
            "focus_window",
        ]
        .contains(&action)
        {
            return Err(ToolError::InvalidInput(
                "Unknown desktop action. Whole-screen capture is unavailable; use a target window."
                    .into(),
            ));
        }
        if action != "list_windows"
            && input["window_id"]
                .as_str()
                .is_none_or(|id| id.parse::<isize>().ok().is_none_or(|handle| handle == 0))
        {
            return Err(ToolError::InvalidInput(
                "Use a window_id returned by list_windows".into(),
            ));
        }
        if action == "screenshot_window" && !self.settings.include_screenshot {
            return Err(ToolError::PermissionDenied(
                "Window screenshots are disabled in Computer Use settings".into(),
            ));
        }
        let requested = match input.get("max_nodes") {
            Some(value) => value.as_u64().filter(|n| *n > 0).ok_or_else(|| {
                ToolError::InvalidInput("max_nodes must be a positive integer".into())
            })?,
            None => self.settings.max_nodes as u64,
        };
        Ok(
            json!({"input": input,"max_nodes":requested.min(self.settings.max_nodes as u64),
            "include_screenshot":self.settings.include_screenshot,"screenshot_width":self.settings.screenshot_width}),
        )
    }
}
impl Default for DesktopTool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
#[path = "desktop_windows.rs"]
mod windows;

#[cfg(windows)]
async fn run_windows(request: &Value, home: &std::path::Path) -> Result<String, ToolError> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .env("AX_HOME", home)
        .arg("--ax-computer-use-worker")
        .creation_flags(0x0800_0000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ToolError::Execution("Desktop helper stdin unavailable".into()))?;
    let payload =
        serde_json::to_vec(request).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    let writer = async move {
        stdin.write_all(&payload).await?;
        stdin.shutdown().await
    };
    let (written, output) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(writer, child.wait_with_output())
    })
    .await
    .map_err(|_| {
        ToolError::Execution(
            "Desktop operation timed out; the target application may be unresponsive".into(),
        )
    })?;
    written?;
    let output = output?;
    if !output.status.success() {
        return Err(ToolError::Execution(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let result: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| ToolError::Execution(format!("Invalid desktop helper response: {e}")))?;
    if let Some(error) = result.get("error").and_then(Value::as_str) {
        return Err(ToolError::Execution(error.to_owned()));
    }
    serde_json::to_string(&result).map_err(|e| ToolError::Execution(e.to_string()))
}

/// Internal helper entry. The CLI supplies freshly loaded host configuration,
/// allowing an off switch to revoke access even in an already-running turn.
///
/// # Errors
/// Returns an error when host access is disabled or request input is invalid.
#[cfg(windows)]
pub async fn desktop_worker(mut settings: DesktopSettings) -> Result<(), ToolError> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    tokio::io::stdin()
        .take(1024 * 1024)
        .read_to_end(&mut bytes)
        .await?;
    let payload: Value =
        serde_json::from_slice(&bytes).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    settings.include_screenshot &= payload["include_screenshot"] == true;
    settings.screenshot_width = settings.screenshot_width.min(
        u32::try_from(payload["screenshot_width"].as_u64().unwrap_or(1280)).unwrap_or(u32::MAX),
    );
    settings.max_nodes = settings
        .max_nodes
        .min(usize::try_from(payload["max_nodes"].as_u64().unwrap_or(1200)).unwrap_or(usize::MAX));
    let request = DesktopTool::with_settings(settings).request(&payload["input"])?;
    let mut request = request;
    request["resolve_only"] = payload["resolve_only"].clone();
    request["authorized_app"] = payload["authorized_app"].clone();
    // No native object crosses threads, no provider/session initialization.
    let result = windows::run(&request).unwrap_or_else(|error| json!({"error":error.to_string()}));
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|e| ToolError::Execution(e.to_string()))?
    );
    Ok(())
}

#[async_trait]
impl Tool for DesktopTool {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::AuthorizedHost
    }
    fn name(&self) -> &'static str {
        "desktop"
    }
    fn description(&self) -> &'static str {
        "Read and operate Windows application windows using accessibility controls. Opt-in only. Read a bounded UI tree, focus windows, invoke controls, enter text, send keys or use target-window coordinates. Optional window screenshots omit password-bearing windows."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","required":["action"],"properties":{
            "action":{"type":"string","enum":["list_windows","read_window","screenshot_window","focus_window","click_element","type_text","press_key","move_mouse","mouse_click"]},
            "window_id":{"type":"string","description":"Window handle from list_windows; required except for list_windows"},
            "element_id":{"type":"string","description":"Element ID from read_window; required for click_element; type_text otherwise uses the focused element in the target window"},
            "include_hidden":{"type":"boolean"},"max_nodes":{"type":"integer","minimum":1,"maximum":self.settings.max_nodes},
            "text":{"type":"string"},"key":{"type":"string","description":"Return, Escape, Tab, Backspace, Delete, arrows, Home, End, PageUp, PageDown, F1-F12 or one letter/digit"},
            "modifiers":{"type":"array","items":{"type":"string","enum":["ctrl","shift","alt"]}},
            "x":{"type":"integer","description":"Screen coordinate inside target window"},"y":{"type":"integer"},
            "button":{"type":"string","enum":["left","right","middle"]}
        }})
    }
    fn capability(&self, _: &Value) -> Capability {
        Capability::ComputerUse
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::RequiresApproval
    }
    fn resources(&self, _: &Value) -> Vec<crate::ResourceAccess> {
        vec![crate::ResourceAccess::write(crate::Resource::Named(
            "desktop:global".into(),
        ))]
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        match self.execute_output_authorized(input, &[], None).await? {
            crate::ToolOutput::Text(text) => Ok(text),
            crate::ToolOutput::Image { description, .. } => Ok(description),
        }
    }
    async fn host_access(
        &self,
        input: &Value,
    ) -> Result<Option<crate::HostAccessRequest>, ToolError> {
        let mut request = self.request(input)?;
        if input["action"] == "list_windows" {
            return Ok(None);
        }
        #[cfg(windows)]
        {
            request["resolve_only"] = json!(true);
            let resolved: Value =
                serde_json::from_str(&run_windows(&request, self.host.home()).await?)
                    .map_err(|e| ToolError::Execution(e.to_string()))?;
            let target = crate::HostTarget {
                surface: crate::HostSurface::Computer,
                id: resolved["app_id"]
                    .as_str()
                    .ok_or_else(|| ToolError::Execution("Application identity unavailable".into()))?
                    .to_owned(),
                label: resolved["app_name"]
                    .as_str()
                    .unwrap_or("application")
                    .to_owned(),
            };
            Ok(Some(crate::HostAccessRequest {
                decision: self.host.decision(&target)?,
                target,
            }))
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            Err(ToolError::Execution(
                "Computer Use currently supports native Windows only".into(),
            ))
        }
    }
    async fn execute_output_authorized(
        &self,
        input: Value,
        _profiles: &[crate::PermissionProfile],
        authorization: Option<&crate::HostAuthorization>,
    ) -> Result<crate::ToolOutput, ToolError> {
        let mut request = self.request(&input)?;
        #[cfg(windows)]
        {
            if input["action"] != "list_windows" {
                let access = self
                    .host_access(&input)
                    .await?
                    .ok_or_else(|| ToolError::PermissionDenied("Missing app identity".into()))?;
                if let Some(authorization) = authorization {
                    if authorization.target() != &access.target {
                        return Err(ToolError::PermissionDenied(
                            "Window application changed; request access again".into(),
                        ));
                    }
                    self.host.grant(authorization)?;
                } else if access.decision != crate::PermissionDecision::Allow {
                    return Err(ToolError::PermissionDenied(format!(
                        "App access {:?}: {}",
                        access.decision, access.target.id
                    )));
                }
                request["authorized_app"] = json!(access.target.id);
            }
            let result = run_windows(&request, self.host.home()).await?;
            let value =
                serde_json::from_str(&result).map_err(|e| ToolError::Execution(e.to_string()))?;
            crate::host_access::host_output(&value)
        }
        #[cfg(not(windows))]
        {
            let _ = (request, authorization);
            Err(ToolError::Execution(
                "Computer Use currently supports native Windows only".into(),
            ))
        }
    }
    fn guidance(&self) -> Option<&'static str> {
        Some(
            "Enumerate windows, then read_window for accessibility IDs. Re-read after changes; stale IDs fail. Prefer click_element/type_text over coordinates. Coordinate/key actions require the target window in the foreground; focus_window explicitly changes focus. A truncated tree reports its limit. Screenshots may be withheld for passwords or unavailable for a renderer; use the UI tree. Configuration cannot be changed through this tool.",
        )
    }
}
