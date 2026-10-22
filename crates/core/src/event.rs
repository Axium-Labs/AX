//! The runtime event stream, plus the tool-activity payloads it carries.
//!
//! Events are the kernel's only output channel: the composition root renders
//! them, the ACP adapter forwards them, and telemetry records them. Nothing
//! here depends on a terminal.

use serde_json::Value;
use tool::{ToolError, ToolOutput};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelConnectionStatus {
    Connecting,
    Retrying,
    Connected,
    Failed,
}

/// A delegated run's public activity, kept separate from controller output.
/// The host's run descriptor is for trusted history recovery, not model context.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SubagentActivity {
    pub id: String,
    pub parent_id: Option<String>,
    pub task: String,
    pub status: String,
    pub run: Option<crate::ChildRun>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub event: Option<Box<AgentEvent>>,
}

impl SubagentActivity {
    #[must_use]
    pub fn running(id: String, task: String, run: Option<crate::ChildRun>) -> Self {
        Self {
            id,
            parent_id: None,
            task,
            status: "running".into(),
            run,
            summary: None,
            error: None,
            event: None,
        }
    }

    /// Descendants retain their own identity and acquire only a missing parent.
    #[must_use]
    pub fn observe(&self, event: AgentEvent) -> Option<AgentEvent> {
        match event {
            AgentEvent::SubagentActivity { mut activity } => {
                activity.parent_id.get_or_insert_with(|| self.id.clone());
                Some(AgentEvent::SubagentActivity { activity })
            }
            event @ (AgentEvent::ContentDelta { .. }
            | AgentEvent::ToolStarted { .. }
            | AgentEvent::ToolFinished { .. }
            | AgentEvent::ModelConnection { .. }) => {
                let mut activity = self.clone();
                activity.event = Some(Box::new(event));
                Some(AgentEvent::SubagentActivity {
                    activity: Box::new(activity),
                })
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn into_event(self) -> AgentEvent {
        AgentEvent::SubagentActivity {
            activity: Box::new(self),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    SubagentActivity {
        activity: Box<SubagentActivity>,
    },
    SessionRotated {
        goal_id: String,
        previous_session: String,
        session: String,
    },
    Continuation {
        goal_id: Option<String>,
        step: usize,
        continuation: crate::TurnContinuation,
    },
    Completion {
        goal_id: Option<String>,
        completion: String,
        model_steps: usize,
        goal_model_requests: usize,
        tools: usize,
        evaluator: Option<String>,
    },
    GoalEvaluated {
        goal_id: Option<String>,
        evaluator: String,
        allowed: bool,
        model_requests: usize,
        decision: crate::GoalDecision,
    },
    GoalUpdated {
        goal_id: Option<String>,
        goal: Box<crate::Goal>,
    },
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
    ModelConnection {
        status: ModelConnectionStatus,
        attempt: usize,
        max_attempts: usize,
        error: Option<String>,
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
    /// The run suspended on `request_user_input`; the frontend shows the
    /// question and turns the next submission into its answer.
    UserQuestion {
        question: Box<crate::UserQuestion>,
    },
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
        "search" => {
            let scope = if field("path").is_empty() {
                field("root")
            } else {
                field("path")
            };
            format!("searching '{}' in {}", field("query"), scope)
        }
        "find_files" | "glob" => {
            let scope = if field("path").is_empty() {
                field("root")
            } else {
                field("path")
            };
            format!("finding {} in {}", field("pattern"), scope)
        }
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
