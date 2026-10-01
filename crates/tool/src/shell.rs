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

struct ChildShell {
    context: crate::RunContext,
}

fn shell_description() -> &'static str {
    if cfg!(windows) {
        "Run commands using Windows PowerShell 5.1 (powershell.exe), platform=windows. Use PowerShell syntax: no bash heredocs (python - <<'PY'), && or ||. Use a PowerShell here-string piped to python, or python -c; run dependent commands separately. Shell execution requires approval."
    } else {
        "Run commands using POSIX sh, platform=unix. Shell execution requires approval."
    }
}

fn validate_command(command: &str, windows: bool) -> Result<(), ToolError> {
    if command.trim().is_empty() {
        return Err(ToolError::InvalidInput("command must not be empty".into()));
    }
    if windows {
        // Check operators outside quotes; quoted Python/string contents remain valid.
        let mut quote = None;
        let mut chars = command.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '`' {
                chars.next();
                continue;
            }
            if let Some(current) = quote {
                if ch == current {
                    quote = None;
                }
                continue;
            }
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                continue;
            }
            if matches!(ch, '&' | '|' | '<') && chars.peek() == Some(&ch) {
                return Err(ToolError::InvalidInput("platform=windows; shell=Windows PowerShell 5.1: bash heredocs, && and || are unsupported. Use PowerShell here-strings or separate commands.".into()));
            }
        }
    }
    Ok(())
}

#[async_trait]
impl Tool for ChildShell {
    fn name(&self) -> &'static str {
        "shell"
    }
    fn description(&self) -> &str {
        shell_description()
    }
    fn input_schema(&self) -> Value {
        ShellTool.input_schema()
    }
    fn capability(&self, input: &Value) -> crate::Capability {
        ShellTool.capability(input)
    }
    fn safety(&self, input: &Value) -> SafetyLevel {
        ShellTool.safety(input)
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        execute_shell(input, Some(&self.context)).await
    }
}

#[derive(Deserialize)]
struct ShellInput {
    command: String,
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for ShellTool {
    fn fork_for_run(&self, context: &crate::RunContext) -> Option<std::sync::Arc<dyn Tool>> {
        Some(std::sync::Arc::new(ChildShell {
            context: context.clone(),
        }))
    }
    fn name(&self) -> &'static str {
        "shell"
    }

    fn description(&self) -> &str {
        shell_description()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": shell_description() }
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
        execute_shell(input, None).await
    }
}

async fn execute_shell(
    input: Value,
    context: Option<&crate::RunContext>,
) -> Result<String, ToolError> {
    let input: ShellInput = serde_json::from_value(input)
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    validate_command(&input.command, cfg!(windows))?;

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

    if let Some(context) = context {
        command
            .current_dir(&context.cwd)
            .env("AX_HOME", &context.state_dir)
            .env("AX_SESSION_ID", &context.session_id)
            .env("AX_MEMORY_SCOPE", &context.memory_scope);
    }
    let output = command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| ToolError::Execution(error.to_string()))?;
    let stdout = decode(&output.stdout);
    let stderr = decode(&output.stderr);
    let text = format!(
        "exit_code: {}\nstdout:\n{}\nstderr:\n{}",
        output.status.code().unwrap_or(-1),
        stdout,
        stderr
    );
    if output.status.success() {
        Ok(text)
    } else {
        Err(ToolError::Execution(text))
    }
}

#[cfg(test)]
mod tests {
    use super::decode;
    #[tokio::test]
    async fn nonzero_exit_is_a_tool_error() {
        use crate::Tool;
        let error = super::ShellTool
            .execute(serde_json::json!({"command":"exit 7"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exit_code: 7"));
    }

    #[test]
    fn windows_schema_and_validation_expose_real_shell_and_reject_bash_operators() {
        assert!(super::validate_command("python - <<'PY'\nprint(1)\nPY", true).is_err());
        assert!(super::validate_command("python -c 'print(1)' && echo done", true).is_err());
        assert!(super::validate_command("echo failed || exit 1", true).is_err());
        assert!(super::validate_command(r#"python -c "print('a && b')""#, true).is_ok());
        assert!(super::validate_command("@'\nprint(1)\n'@ | python -", true).is_ok());
        assert!(super::validate_command("true && echo ok", false).is_ok());
        if cfg!(windows) {
            use crate::Tool;
            assert!(super::ShellTool.description().contains("platform=windows"));
            assert!(
                super::ShellTool.input_schema()["properties"]["command"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("PowerShell 5.1")
            );
        }
    }

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
