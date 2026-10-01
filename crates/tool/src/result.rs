//! Lossless stored results with a bounded, model-facing projection.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Clone, Default)]
pub struct ResultReader(pub Arc<RwLock<HashMap<String, String>>>);

#[async_trait::async_trait]
impl crate::Tool for ResultReader {
    fn name(&self) -> &'static str {
        "tool_output"
    }
    fn description(&self) -> &'static str {
        "Read retained raw output for a previous call_id, using a small 1-based start_line/end_line range. Use only if its compact result omitted needed details."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{"call_id":{"type":"string"},"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1}},"required":["call_id","start_line","end_line"],"additionalProperties":false})
    }
    fn safety(&self, _: &Value) -> crate::SafetyLevel {
        crate::SafetyLevel::Safe
    }
    fn capability(&self, _: &Value) -> crate::Capability {
        crate::Capability::FilesystemRead
    }
    fn resources(&self, _: &Value) -> Vec<crate::ResourceAccess> {
        vec![]
    }
    async fn execute(&self, input: Value) -> Result<String, crate::ToolError> {
        let id = input["call_id"].as_str().unwrap_or_default();
        let start = usize::try_from(input["start_line"].as_u64().unwrap_or(0)).unwrap_or(0);
        let end = usize::try_from(input["end_line"].as_u64().unwrap_or(0)).unwrap_or(0);
        if start == 0 || end < start || end - start >= 200 {
            return Err(crate::ToolError::InvalidInput(
                "request 1..200 lines".into(),
            ));
        }
        let store = self
            .0
            .read()
            .map_err(|_| crate::ToolError::Execution("raw result store unavailable".into()))?;
        let raw = store.get(id).ok_or_else(|| {
            crate::ToolError::InvalidInput(format!(
                "unknown call_id {id}; use an earlier tool call ID"
            ))
        })?;
        let pretty = serde_json::from_str::<Value>(raw)
            .and_then(|v| serde_json::to_string_pretty(&v))
            .unwrap_or_else(|_| raw.clone());
        Ok(pretty
            .lines()
            .enumerate()
            .skip(start - 1)
            .take(end - start + 1)
            .map(|(i, line)| format!("{}: {line}", i + 1))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    /// Explicit goal-wide failures stop queue consumption; ordinary errors are local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_blocker: Option<String>,
    pub status: String,
    pub summary: String,
    pub diagnostics: Vec<Value>,
    pub raw_output: String,
    pub truncated: bool,
}

impl ToolResult {
    #[must_use]
    pub fn from_legacy(raw_output: String) -> Self {
        let first = raw_output.lines().next().unwrap_or_default();
        let exit_failed = first
            .strip_prefix("exit_code:")
            .and_then(|v| v.trim().parse::<i32>().ok())
            .is_some_and(|code| code != 0);
        let failed = exit_failed
            || [
                "tool execution failed:",
                "invalid tool input:",
                "permission denied:",
                "unknown tool:",
                "tool execution interrupted",
            ]
            .iter()
            .any(|prefix| raw_output.starts_with(prefix));
        Self::new(!failed, raw_output)
    }
    #[must_use]
    pub fn new(success: bool, raw_output: String) -> Self {
        let parsed = serde_json::from_str::<Value>(&raw_output).ok();
        let diagnostics = parsed
            .as_ref()
            .and_then(|v| v.get("diagnostics"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                raw_output
                    .lines()
                    .filter(|line| {
                        let lower = line.to_lowercase();
                        lower.contains("error")
                            || lower.contains("failed")
                            || lower.contains("conflict")
                    })
                    .take(12)
                    .map(|line| json!({"message":line}))
                    .collect()
            });
        Self { global_blocker: None, status: if success { "success" } else { "error" }.into(),
            summary: if success { "Operation succeeded" } else { "Operation failed; repair the reported diagnostic locally and run the smallest relevant check before broader tests" }.into(),
            diagnostics, raw_output, truncated: parsed.as_ref().and_then(|v| v["truncated"].as_bool()).unwrap_or(false) }
    }

    /// Budget is supplied by `ContextBudget`, never by the tool or provider.
    #[must_use]
    pub fn model_view(&self, max_chars: usize) -> String {
        let pretty = serde_json::from_str::<Value>(&self.raw_output)
            .and_then(|value| serde_json::to_string_pretty(&value))
            .unwrap_or_else(|_| self.raw_output.clone());
        // Successful compiler warning floods are redundant; failures retain diagnostics.
        let lines: Vec<_> = pretty
            .lines()
            .filter(|line| !line.trim_start().starts_with("warning:"))
            .collect();
        let cleaned = lines.join("\n");
        let clipped: String = cleaned.chars().take(max_chars).collect();
        let diagnostic_chars = max_chars / self.diagnostics.len().clamp(1, 12);
        let diagnostics:Vec<_>=self.diagnostics.iter().take(12).map(|diagnostic| {
            let text=diagnostic.to_string();
            if text.chars().count()>diagnostic_chars { json!({"message":text.chars().take(diagnostic_chars).collect::<String>(),"truncated":true}) } else { diagnostic.clone() }
        }).collect();
        json!({"global_blocker":self.global_blocker,"status":self.status,"summary":self.summary,"diagnostics":diagnostics,
            "output":clipped,"truncated":self.truncated || clipped.len()<cleaned.len() || cleaned != pretty,
            "raw_output_available":true}).to_string()
    }
}

/// Report nearby names without a recursive workspace search.
pub fn path_error(path: &std::path::Path, error: &std::io::Error) -> crate::ToolError {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let candidates: Vec<_> = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .take(12)
        .map(|entry| entry.path().display().to_string())
        .collect();
    crate::ToolError::Execution(json!({"diagnostics":[{"kind":"path_error","path":path,"message":error.to_string(),"candidate_paths":candidates}]}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_shell_errors_remain_errors() {
        assert_eq!(
            ToolResult::from_legacy("exit_code: 1\nfailed".into()).status,
            "error"
        );
        assert_eq!(
            ToolResult::from_legacy("exit_code: 0\nok".into()).status,
            "success"
        );
    }
    #[test]
    fn retains_raw_and_critical_errors_when_projection_is_short() {
        let raw = format!(
            "{}\nerror: broken at src/main.rs:42",
            "warning: redundant\n".repeat(500)
        );
        let result = ToolResult::new(false, raw.clone());
        let view: Value = serde_json::from_str(&result.model_view(80)).unwrap();
        assert_eq!(result.raw_output, raw);
        assert_eq!(view["status"], "error");
        assert_eq!(view["truncated"], true);
        assert!(view["diagnostics"].to_string().contains("main.rs:42"));
        assert!(!view.to_string().contains("warning: redundant"));
    }
}
