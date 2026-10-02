//! The runtime event stream, plus the tool-activity payloads it carries.
//!
//! Events are the kernel's only output channel: the composition root renders
//! them, the ACP adapter forwards them, and telemetry records them. Nothing
//! here depends on a terminal.

use serde_json::Value;
use tool::{ToolError, ToolOutput};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    SubagentStarted {
        id: String,
    },
    SubagentProgress {
        id: String,
        phase: String,
    },
    SubagentCompleted {
        id: String,
    },
    SubagentFailed {
        id: String,
        error: String,
    },
    SubagentCancelled {
        id: String,
    },
    TurnStarted,
    ModelStarted {
        provider: String,
        model: String,
    },
    ContentDelta {
        delta: String,
    },
    /// Streaming reasoning / chain-of-thought, shown ahead of the answer.
    ThinkingDelta {
        delta: String,
    },
    ToolStarted {
        id: String,
        name: String,
        detail: String,
        input: Value,
    },
    ToolFinished {
        id: String,
        name: String,
        success: bool,
        diagnostics: Vec<tool::FetchError>,
        result: tool::ToolResult,
    },
    TurnFinished,
    ContextCompressed {
        removed_messages: usize,
        estimated_tokens_before: usize,
        estimated_tokens_after: usize,
        tokens_freed: usize,
        compression_ratio: f64,
        cleanup_tier: &'static str,
        semantic_called: bool,
        tool_outputs_reduced: usize,
        tool_outputs_removed: usize,
        recent_raw_tokens: usize,
    },
}

/// Diagnostics carried by [`AgentEvent::ToolFinished`] when a `web` call fails
/// outright or only partially succeeds.
pub(crate) fn fetch_diagnostics(
    name: &str,
    result: &Result<ToolOutput, ToolError>,
) -> Vec<tool::FetchError> {
    match result {
        Err(ToolError::WebFetch(errors)) => errors.clone(),
        Ok(ToolOutput::Text(text)) if name == "web" => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|value| serde_json::from_value(value["errors"].clone()).ok())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// One-line description of what a tool call is doing, carried by
/// [`AgentEvent::ToolStarted`] so the frontend can show progress without
/// interpreting tool arguments itself.
#[must_use]
pub fn tool_activity(name: &str, input: &Value) -> String {
    let field = |key: &str| input.get(key).and_then(Value::as_str).unwrap_or("");
    let detail = match name {
        "search" => format!("searching '{}' in {}", field("query"), field("path")),
        "filesystem" => format!("{} {}", field("operation"), field("path")),
        "patch" => format!("editing {}", field("path")),
        "shell" => format!("running {}", field("command").lines().next().unwrap_or("")),
        "web" => {
            // `queries`/`urls` drive batched calls; the singular keys remain as
            // legacy aliases and also name the unit in the description.
            let (list_key, unit) = if field("operation") == "search" {
                ("queries", "query")
            } else {
                ("urls", "url")
            };
            let list = input.get(list_key).and_then(Value::as_array);
            let count = list
                .map_or(0, Vec::len)
                .max(usize::from(!field(unit).is_empty()));
            let first = list
                .and_then(|items| items.first())
                .and_then(Value::as_str)
                .unwrap_or_else(|| field(unit));
            format!(
                "{} {count} {}: {first}",
                field("operation"),
                if count == 1 { unit } else { list_key }
            )
        }
        "mcp" => format!("{} {} {}", field("action"), field("server"), field("tool")),
        _ if name.starts_with("mcp__") => format!("calling {name}"),
        _ => format!("calling {name}"),
    };
    detail.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::tool_activity;
    use serde_json::json;

    #[test]
    fn fetch_diagnostics_survive_both_partial_and_total_failure() {
        use super::{ToolError, ToolOutput, fetch_diagnostics};
        let value = serde_json::json!({
            "url": "https://example.test/full",
            "kind": "connect",
            "reason": "connection failed",
            "error": "outer: root",
            "source_chain": ["outer", "root"]
        });
        let diagnostic = serde_json::from_value(value.clone()).unwrap();
        let failed = Err(ToolError::WebFetch(vec![diagnostic]));
        let partial = Ok(ToolOutput::Text(
            serde_json::json!({"errors": [value]}).to_string(),
        ));
        for result in [failed, partial] {
            let diagnostics = fetch_diagnostics("web", &result);
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].source_chain, ["outer", "root"]);
            assert_eq!(diagnostics[0].url, "https://example.test/full");
        }
        let search = Ok(ToolOutput::Text(
            serde_json::json!({"errors": [{"query": "q", "error": "search failed"}]}).to_string(),
        ));
        assert!(fetch_diagnostics("web", &search).is_empty());
    }

    #[test]
    fn describes_read_search_edit_and_command() {
        assert!(
            tool_activity("search", &json!({"query":"needle","path":"src"})).contains("needle")
        );
        assert!(
            tool_activity(
                "filesystem",
                &json!({"operation":"read","path":"README.md"})
            )
            .contains("read README.md")
        );
        assert!(
            tool_activity("patch", &json!({"path":"src/main.rs"})).contains("editing src/main.rs")
        );
        assert!(
            tool_activity("shell", &json!({"command":"cargo test\nother"}))
                .contains("running cargo test")
        );
    }

    #[test]
    fn describes_batched_web_calls() {
        assert_eq!(
            tool_activity(
                "web",
                &json!({"operation":"search","queries":["rust 1.94","rust notes"]})
            ),
            "search 2 queries: rust 1.94"
        );
        assert_eq!(
            tool_activity("web", &json!({"operation":"fetch","url":"https://ax.test"})),
            "fetch 1 url: https://ax.test"
        );
        assert_eq!(
            tool_activity("web", &json!({"operation":"search","query":"legacy"})),
            "search 1 query: legacy"
        );
    }
}
