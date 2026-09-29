use std::borrow::Cow;
use std::process::Stdio;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::{SafetyLevel, Tool, ToolError};

/**
 * Child output as text.
 *
 * UTF-8 is the common case and is returned as-is. A Windows `PowerShell` writes to a
 * pipe in the console code page instead — GBK on a Chinese system — and those
 * bytes are not valid UTF-8, so a lossy decode turns every Chinese message into
 * `U+FFFD`. Fall back to the system code page before giving up.
 */
fn decode(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => native_decode(bytes).map_or_else(|| String::from_utf8_lossy(bytes), Cow::Owned),
    }
}

/// Decodes with the console code page, then the ANSI code page. `None` on any
/// other platform or when the bytes are not text in either.
#[cfg(windows)]
fn native_decode(bytes: &[u8]) -> Option<String> {
    const CP_UTF8: u32 = 65001;
    const CP_ACP: u32 = 0;
    const MB_ERR_INVALID_CHARS: u32 = 0x0000_0008;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleOutputCP() -> u32;
        fn MultiByteToWideChar(
            code_page: u32,
            flags: u32,
            source: *const u8,
            source_len: i32,
            wide: *mut u16,
            wide_len: i32,
        ) -> i32;
    }

    /// Converts with one code page, rejecting bytes it cannot map.
    fn convert(code_page: u32, bytes: &[u8]) -> Option<String> {
        if code_page == CP_UTF8 {
            return None; // Already tried as UTF-8.
        }
        let source_len = i32::try_from(bytes.len()).ok()?;
        // SAFETY: `bytes` and `wide` are valid for the lengths passed, and
        // `MultiByteToWideChar` only writes within `wide`.
        unsafe {
            let needed = MultiByteToWideChar(
                code_page,
                MB_ERR_INVALID_CHARS,
                bytes.as_ptr(),
                source_len,
                std::ptr::null_mut(),
                0,
            );
            if needed <= 0 {
                return None;
            }
            let mut wide = vec![0u16; usize::try_from(needed).ok()?];
            let written = MultiByteToWideChar(
                code_page,
                MB_ERR_INVALID_CHARS,
                bytes.as_ptr(),
                source_len,
                wide.as_mut_ptr(),
                needed,
            );
            if written <= 0 {
                return None;
            }
            String::from_utf16(&wide[..usize::try_from(written).ok()?]).ok()
        }
    }

    if bytes.is_empty() {
        return Some(String::new());
    }
    // SAFETY: no arguments, no preconditions.
    let console = unsafe { GetConsoleOutputCP() };
    convert(console, bytes).or_else(|| convert(CP_ACP, bytes))
}

#[cfg(not(windows))]
fn native_decode(_bytes: &[u8]) -> Option<String> {
    None
}

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

    fn capability(&self, _input: &Value) -> crate::Capability {
        crate::Capability::Shell
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
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| ToolError::Execution(error.to_string()))?;
        let stdout = decode(&output.stdout);
        let stderr = decode(&output.stderr);
        Ok(format!(
            "exit_code: {}\nstdout:\n{}\nstderr:\n{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn leaves_utf8_untouched() {
        assert_eq!(decode("ls: 没有那个文件".as_bytes()), "ls: 没有那个文件");
        assert_eq!(decode(b""), "");
    }

    /// Windows shells write GBK to a pipe; the old lossy decode turned every
    /// Chinese message into U+FFFD.
    #[cfg(windows)]
    #[test]
    fn decodes_the_console_code_page() {
        let gbk: Vec<u8> = vec![0xC4, 0xE3, 0xBA, 0xC3]; // 你好 in GBK
        assert!(String::from_utf8_lossy(&gbk).contains('\u{FFFD}'));
        assert_eq!(decode(&gbk), "你好");
    }

    #[test]
    fn survives_bytes_that_are_not_text() {
        let broken: [u8; 3] = [0xff, 0xfe, 0x41];
        assert!(!decode(&broken).is_empty());
    }
}
