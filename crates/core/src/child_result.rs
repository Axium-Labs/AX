//! Structured child receipts.
//!
//! A child used to hand the controller a single `{success, output}` pair, which
//! forced the controller either to re-read the child transcript or to guess.
//! [`ChildResult`] carries the machine-checkable record instead, and the model
//! only ever sees the compact [`ChildResult::model_summary`] projection; the
//! full receipt is persisted and read back on demand through the
//! [`TOOL_NAME`] control tool.
//!
//! Persistence follows the task queue: the receipt map is durable orchestration
//! state (its own marker, its own `agent_states` row), never conversation
//! context, so it survives compression and resume without inflating the
//! controller's context window.

use std::collections::BTreeMap;
use std::time::Instant;

use model::{FunctionSpec, Message, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AgentEvent;

pub const STATE_PREFIX: &str = "[ax-child-results]\n";
pub const TOOL_NAME: &str = "child_result";
/// Retained receipts per controller goal. Older receipts stay in the child's
/// own session; this bound only limits the controller-side index.
pub const MAX_RETAINED: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStatus {
    Completed,
    Failed,
    TimedOut,
    Cancelled,
    WaitingForUser,
}

impl ChildStatus {
    #[must_use]
    pub const fn success(self) -> bool {
        matches!(self, Self::Completed)
    }

    #[must_use]
    pub const fn terminal(self) -> bool {
        !matches!(self, Self::WaitingForUser)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::TimedOut => "timed out",
            Self::Cancelled => "cancelled",
            Self::WaitingForUser => "waiting for user",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildMetrics {
    pub wall_time_ms: u64,
    pub model_rounds: usize,
    pub tool_calls: usize,
    pub failed_tool_calls: usize,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: String,
    /// `created`, `modified` or `deleted`.
    pub change: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStat {
    pub files: usize,
    pub insertions: u64,
    pub deletions: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validation {
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub path: String,
    pub kind: String,
}

/// One child's complete receipt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildResult {
    pub child_id: String,
    pub task_id: String,
    pub status: ChildStatus,
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<ChangedFile>,
    #[serde(default)]
    pub diff_stat: DiffStat,
    #[serde(default)]
    pub diagnostics: Vec<String>,
    #[serde(default)]
    pub validation: Vec<Validation>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub failure_reason: Option<String>,
    #[serde(default)]
    pub continuation_hint: Option<String>,
    #[serde(default)]
    pub metrics: ChildMetrics,
}

impl ChildResult {
    #[must_use]
    pub fn new(
        child_id: impl Into<String>,
        task_id: impl Into<String>,
        status: ChildStatus,
    ) -> Self {
        Self {
            child_id: child_id.into(),
            task_id: task_id.into(),
            status,
            summary: String::new(),
            findings: Vec::new(),
            changed_files: Vec::new(),
            diff_stat: DiffStat::default(),
            diagnostics: Vec::new(),
            validation: Vec::new(),
            artifacts: Vec::new(),
            failure_reason: None,
            continuation_hint: None,
            metrics: ChildMetrics::default(),
        }
    }

    #[must_use]
    pub fn failed(
        child_id: impl Into<String>,
        task_id: impl Into<String>,
        status: ChildStatus,
        reason: impl Into<String>,
    ) -> Self {
        let reason = reason.into();
        let mut result = Self::new(child_id, task_id, status);
        result.failure_reason = Some(reason.clone());
        result.summary.clone_from(&reason);
        result.diagnostics.push(reason);
        result.continuation_hint = Some(match status {
            ChildStatus::TimedOut => {
                "Re-dispatch with a narrower task input or a larger child timeout.".into()
            }
            ChildStatus::Cancelled => "Re-dispatch if the work is still required.".into(),
            _ => "Inspect the diagnostics, then retry with a narrower task input.".into(),
        });
        result
    }

    /// The compact projection handed to the controller model. Everything else
    /// is available through [`TOOL_NAME`].
    #[must_use]
    pub fn model_summary(&self) -> String {
        let files = if self.changed_files.is_empty() {
            "none".to_owned()
        } else {
            self.changed_files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let validation = if self.validation.is_empty() {
            "not run".to_owned()
        } else {
            self.validation
                .iter()
                .map(|entry| {
                    format!(
                        "{} ({})",
                        entry.command,
                        if entry.success { "ok" } else { "failed" }
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        format!(
            "Child {} {}:\n- root cause: {}\n- relevant files: {}\n- validation: {}\n- suggested next step: {}\n(child_result tool: `{}` for the full receipt)",
            self.child_id,
            self.status.label(),
            bounded(
                self.failure_reason
                    .as_deref()
                    .unwrap_or(self.summary.as_str()),
                320
            ),
            files,
            validation,
            self.continuation_hint
                .as_deref()
                .unwrap_or("continue with the controller plan"),
            self.child_id
        )
    }

    /// Full receipt rendering for the read tool, restricted to one aspect.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        !self.changed_files.is_empty()
    }
}

/// Parse a durable receipt, accepting the legacy `{success, output}` shape so a
/// child finished by an older build is still recognised instead of re-run.
#[must_use]
pub fn from_durable_json(json: &str) -> Option<ChildResult> {
    if let Ok(result) = serde_json::from_str::<ChildResult>(json) {
        return Some(result);
    }
    let legacy: LegacyReceipt = serde_json::from_str(json).ok()?;
    let status = if legacy.success {
        ChildStatus::Completed
    } else {
        ChildStatus::Failed
    };
    let mut result = ChildResult::new(String::new(), String::new(), status);
    result.summary.clone_from(&legacy.output);
    result.failure_reason = (!legacy.success).then_some(legacy.output);
    result.continuation_hint =
        (!legacy.success).then(|| "Inspect the diagnostics, then retry.".to_owned());
    Some(result)
}

/// Legacy child receipts stored `{success, output}` instead of a full record.
#[derive(Deserialize)]
struct LegacyReceipt {
    success: bool,
    #[serde(default)]
    output: String,
}

fn bounded(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

/// Insert a receipt and keep the controller-side index bounded. The eviction
/// key is the smallest `child_id`, so the bound is deterministic.
pub(crate) fn store(receipts: &mut BTreeMap<String, ChildResult>, receipt: ChildResult) {
    let id = receipt.child_id.clone();
    receipts.insert(id, receipt);
    while receipts.len() > MAX_RETAINED {
        let Some(oldest) = receipts.keys().next().cloned() else {
            break;
        };
        receipts.remove(&oldest);
    }
}

pub(crate) fn snapshot(receipts: &BTreeMap<String, ChildResult>) -> Option<Message> {
    if receipts.is_empty() {
        return None;
    }
    Some(Message::system(format!(
        "{STATE_PREFIX}{}",
        serde_json::to_string(receipts).unwrap_or_default()
    )))
}

/// Latest durable receipt map, with every receipt marker removed from the
/// message vector so a forked or resumed kernel starts from one clean copy.
pub(crate) fn restore(messages: &mut Vec<Message>) -> BTreeMap<String, ChildResult> {
    let receipts = messages
        .iter()
        .rev()
        .filter(|message| message.role == model::Role::System)
        .find_map(|message| {
            message
                .content
                .strip_prefix(STATE_PREFIX)
                .and_then(|json| serde_json::from_str::<BTreeMap<String, ChildResult>>(json).ok())
        })
        .unwrap_or_default();
    messages.retain(|message| {
        message.role != model::Role::System || !message.content.starts_with(STATE_PREFIX)
    });
    receipts
}

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: TOOL_NAME.into(),
            description: "Read the full receipt of a child that already ran. The compact child summary you were given omits findings, diagnostics, changed files, diff statistics, validation detail and metrics; fetch them here instead of re-running the child. Omit child_id to list every receipt. Call it alone in a round.".into(),
            parameters: json!({"type":"object","properties":{
                "child_id":{"type":"string","description":"Receipt to read; omit to list all receipts"},
                "aspect":{"type":"string","enum":["full","findings","diagnostics","artifacts","diff","validation","metrics"],"description":"Which part of the receipt to return"}
            }}),
        },
    }
}

pub(crate) fn schema_tokens() -> usize {
    let spec = spec();
    crate::token::estimate_text_tokens(&spec.function.name)
        + crate::token::estimate_text_tokens(&spec.function.description)
        + crate::token::estimate_text_tokens(&spec.function.parameters.to_string())
}

/// # Errors
/// Rejects an unknown child id or an unknown aspect.
pub(crate) fn apply_read(
    receipts: &BTreeMap<String, ChildResult>,
    input: &Value,
) -> Result<String, String> {
    let aspect = input["aspect"].as_str().unwrap_or("full");
    let Some(child_id) = input["child_id"].as_str().filter(|id| !id.is_empty()) else {
        let listing = receipts
            .values()
            .map(|receipt| {
                json!({
                    "child_id": receipt.child_id,
                    "task_id": receipt.task_id,
                    "status": receipt.status,
                    "summary": bounded(&receipt.summary, 240),
                })
            })
            .collect::<Vec<_>>();
        return serde_json::to_string_pretty(&json!({"children": listing}))
            .map_err(|error| error.to_string());
    };
    let receipt = receipts
        .get(child_id)
        .or_else(|| {
            receipts
                .values()
                .find(|receipt| receipt.task_id == child_id)
        })
        .ok_or_else(|| format!("no receipt for child `{child_id}`"))?;
    let value = match aspect {
        "full" => json!(receipt),
        "findings" => json!(receipt.findings),
        "diagnostics" => json!(receipt.diagnostics),
        "artifacts" => json!(receipt.artifacts),
        "diff" => json!({"changed_files": receipt.changed_files, "diff_stat": receipt.diff_stat}),
        "validation" => json!(receipt.validation),
        "metrics" => json!(receipt.metrics),
        other => return Err(format!("unknown receipt aspect `{other}`")),
    };
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

/// Accumulates the structured parts of a child receipt from its event stream.
///
/// The observer is the only place that interprets child events, so the loop
/// stays free of receipt bookkeeping.
pub struct ChildObserver {
    started: Instant,
    pending: std::collections::HashMap<String, (String, Value)>,
    summary: String,
    pub metrics: ChildMetrics,
    pub findings: Vec<String>,
    pub changed_files: Vec<ChangedFile>,
    pub validation: Vec<Validation>,
    pub diagnostics: Vec<String>,
}

impl Default for ChildObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl ChildObserver {
    const MAX_FINDINGS: usize = 8;
    const MAX_DIAGNOSTICS: usize = 16;
    const MAX_VALIDATION: usize = 8;

    #[must_use]
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            pending: std::collections::HashMap::new(),
            summary: String::new(),
            metrics: ChildMetrics::default(),
            findings: Vec::new(),
            changed_files: Vec::new(),
            validation: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::ModelStarted { .. } => {
                self.metrics.model_rounds += 1;
                // The final answer is whatever the last round produced.
                self.summary.clear();
            }
            AgentEvent::ContentDelta { delta } => {
                if self.summary.chars().count() < 4_000 {
                    self.summary.push_str(delta);
                }
            }
            AgentEvent::ToolStarted {
                id, name, input, ..
            } => {
                self.metrics.tool_calls += 1;
                self.pending
                    .insert(id.clone(), (name.clone(), input.clone()));
                self.note_write(name, input);
            }
            AgentEvent::ToolFinished {
                id,
                name,
                success,
                result,
                ..
            } => {
                if !*success {
                    self.metrics.failed_tool_calls += 1;
                }
                let input = self
                    .pending
                    .remove(id)
                    .map_or(Value::Null, |(_, input)| input);
                if *success && is_discovery(name) && self.findings.len() < Self::MAX_FINDINGS {
                    let finding = format!("{name}: {}", bounded(&result.summary, 200));
                    self.findings.push(finding);
                }
                if let Some(command) = verification_command(name, &input) {
                    if self.validation.len() < Self::MAX_VALIDATION {
                        self.validation.push(Validation {
                            command: bounded(&command, 160),
                            success: *success,
                            detail: if *success {
                                String::new()
                            } else {
                                bounded(&result.summary, 240)
                            },
                        });
                    } else if let Some(last) = self.validation.last_mut() {
                        last.success &= *success;
                    }
                }
                if !*success && self.diagnostics.len() < Self::MAX_DIAGNOSTICS {
                    self.diagnostics
                        .push(format!("{name}: {}", bounded(&result.summary, 240)));
                }
                for diagnostic in result
                    .diagnostics
                    .iter()
                    .take(Self::MAX_DIAGNOSTICS.saturating_sub(self.diagnostics.len()))
                {
                    self.diagnostics.push(format!(
                        "{name}: {}",
                        bounded(&diagnostic_text(diagnostic), 240)
                    ));
                }
            }
            _ => {}
        }
    }

    /// Provider that does not stream still produced a final answer.
    pub fn set_output(&mut self, output: &str) {
        if self.summary.trim().is_empty() && !output.trim().is_empty() {
            output.clone_into(&mut self.summary);
        }
    }

    fn note_write(&mut self, name: &str, input: &Value) {
        let path = write_path(name, input);
        let Some(path) = path else {
            return;
        };
        let change = if name == "filesystem" {
            "created"
        } else {
            "modified"
        };
        if self
            .changed_files
            .iter()
            .any(|file| file.path == path && file.change == change)
        {
            return;
        }
        self.changed_files.push(ChangedFile {
            path,
            change: change.to_owned(),
        });
    }

    /// Fold reported provider usage from the child transcript.
    pub fn absorb_usage(&mut self, messages: &[Message]) {
        for message in messages {
            let Some(usage) = &message.usage else {
                continue;
            };
            let reported = usage.get("reported").unwrap_or(usage);
            self.metrics.prompt_tokens = self
                .metrics
                .prompt_tokens
                .saturating_add(token_field(reported, &["prompt_tokens", "input_tokens"]));
            self.metrics.completion_tokens = self.metrics.completion_tokens.saturating_add(
                token_field(reported, &["completion_tokens", "output_tokens"]),
            );
        }
    }

    #[must_use]
    pub fn into_result(
        self,
        child_id: impl Into<String>,
        task_id: impl Into<String>,
        status: ChildStatus,
        failure_reason: Option<String>,
    ) -> ChildResult {
        let mut metrics = self.metrics;
        metrics.wall_time_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let child_id = child_id.into();
        let task_id = task_id.into();
        let artifacts = self
            .changed_files
            .iter()
            .map(|file| Artifact {
                path: file.path.clone(),
                kind: "workspace-file".into(),
            })
            .collect();
        let summary = if self.summary.trim().is_empty() {
            failure_reason.clone().unwrap_or_default()
        } else {
            bounded(&self.summary, 2_048)
        };
        ChildResult {
            child_id,
            task_id,
            status,
            summary,
            findings: self.findings,
            changed_files: self.changed_files,
            diff_stat: DiffStat::default(),
            diagnostics: self.diagnostics,
            validation: self.validation,
            artifacts,
            continuation_hint: None,
            failure_reason,
            metrics,
        }
    }
}

fn token_field(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0)
}

/// Diagnostics are free-form JSON from the tool envelope.
fn diagnostic_text(diagnostic: &Value) -> String {
    diagnostic
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            ["reason", "message", "error", "summary"]
                .iter()
                .find_map(|key| diagnostic.get(*key).and_then(Value::as_str))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| diagnostic.to_string())
}

fn is_discovery(name: &str) -> bool {
    matches!(name, "search" | "find_files" | "glob" | "web")
}

fn write_path(name: &str, input: &Value) -> Option<String> {
    match name {
        "patch" => input["path"].as_str().map(str::to_owned),
        "filesystem" if input["operation"].as_str() == Some("write") => {
            input["path"].as_str().map(str::to_owned)
        }
        _ => None,
    }
}

/// A shell command counts as validation when it runs a known verifier.
fn verification_command(name: &str, input: &Value) -> Option<String> {
    if name != "shell" {
        return None;
    }
    let command = input["command"].as_str()?;
    let first = command.lines().next().unwrap_or("").trim();
    let lower = first.to_lowercase();
    let verifying = [
        "test", "check", "clippy", "fmt", "build", "lint", "tsc", "pytest", "cargo",
    ]
    .iter()
    .any(|verb| lower.contains(verb));
    verifying.then(|| first.to_owned())
}

#[cfg(test)]
#[path = "child_result_tests.rs"]
mod tests;
