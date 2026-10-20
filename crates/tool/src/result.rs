//! Lossless stored results with a bounded, model-facing projection.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
};

#[derive(Clone, Default)]
pub struct ResultReader(pub Arc<RwLock<ResultCache>>);

/// FIFO retained receipts, bounded by the shared file-byte limit (including IDs
/// and serialized metadata). All live, restore and legacy insertions use this gate.
#[derive(Default)]
pub struct ResultCache {
    entries: HashMap<String, RetainedOutput>,
    order: VecDeque<String>,
    bytes: usize,
}

impl ResultCache {
    fn entry_bytes(id: &str, value: &RetainedOutput) -> usize {
        id.len()
            .saturating_add(serde_json::to_vec(value).map_or(usize::MAX, |v| v.len()))
    }

    pub fn get(&self, id: &str) -> Option<&RetainedOutput> {
        self.entries.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &RetainedOutput)> {
        self.entries.iter()
    }

    pub fn insert(&mut self, id: String, value: RetainedOutput) {
        if let Some(old) = self.entries.remove(&id) {
            self.bytes = self.bytes.saturating_sub(Self::entry_bytes(&id, &old));
            self.order.retain(|key| key != &id);
        }
        let size = Self::entry_bytes(&id, &value);
        let limit = usize::try_from(crate::discovery::MAX_FILE_BYTES).unwrap_or(usize::MAX);
        // An individually oversized receipt cannot fit; do not evict useful
        // smaller receipts for an entry that would itself immediately disappear.
        if size > limit {
            return;
        }
        while self.bytes.saturating_add(size) > limit {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(Self::entry_bytes(&oldest, &old));
            }
        }
        self.bytes += size;
        self.order.push_back(id.clone());
        self.entries.insert(id, value);
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = (String, RetainedOutput)>) {
        for (id, value) in entries {
            self.insert(id, value);
        }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&String, &RetainedOutput) -> bool) {
        self.entries.retain(|id, value| keep(id, value));
        self.order.retain(|id| self.entries.contains_key(id));
        self.bytes = self
            .entries
            .iter()
            .map(|(id, value)| Self::entry_bytes(id, value))
            .sum();
    }
}

impl std::ops::Index<&str> for ResultCache {
    type Output = RetainedOutput;
    fn index(&self, id: &str) -> &Self::Output {
        &self.entries[id]
    }
}

impl ResultReader {
    /// Rebuild one cache entry from saved history.
    ///
    /// Saved tool messages may already hold a compacted projection rather than
    /// the original receipt, so a truncated envelope is never allowed to replace
    /// a complete one for the same call. It stays marked `truncated`, which lets
    /// `tool_output` refuse ranges it no longer retains instead of answering an
    /// empty range that looks like missing content.
    pub fn restore(&self, id: &str, result: &ToolResult) {
        let Ok(mut store) = self.0.write() else {
            return;
        };
        let entry = RetainedOutput::from(result);
        if store
            .get(id)
            .is_none_or(|old| old.is_partial() && !entry.is_partial())
        {
            store.insert(id.to_owned(), entry);
        }
    }
}

/// Typed cache metadata is an object; legacy raw cache values remain strings.
/// Never infer source semantics by parsing a tool's raw text as an envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RetainedOutput {
    Verbatim(ToolResult),
    Raw(String),
}

impl From<&ToolResult> for RetainedOutput {
    fn from(result: &ToolResult) -> Self {
        if result.verbatim {
            Self::Verbatim(result.clone())
        } else {
            Self::Raw(result.raw_output.clone())
        }
    }
}

impl RetainedOutput {
    /// A cache entry rebuilt from saved history may already hold a compacted
    /// projection instead of the original receipt.
    #[must_use]
    pub fn is_partial(&self) -> bool {
        matches!(self, Self::Verbatim(result) if result.truncated)
    }
}

impl From<String> for RetainedOutput {
    fn from(raw: String) -> Self {
        Self::Raw(raw)
    }
}

#[async_trait::async_trait]
impl crate::Tool for ResultReader {
    fn execution_boundary(&self) -> crate::ExecutionBoundary {
        crate::ExecutionBoundary::RuntimeOwned
    }
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
        match self.execute_output(input).await? {
            crate::ToolOutput::Text(text) | crate::ToolOutput::Verbatim { text, .. } => Ok(text),
            crate::ToolOutput::Image { description, .. } => Ok(description),
        }
    }
    async fn execute_output(&self, input: Value) -> Result<crate::ToolOutput, crate::ToolError> {
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
        let stored = store.get(id).ok_or_else(|| {
            crate::ToolError::InvalidInput(format!(
                "unknown or evicted call_id {id}; retained output is byte-bounded; read the file again with filesystem.read or rerun the original tool"
            ))
        })?;
        match stored {
            RetainedOutput::Verbatim(result) => {
                let first = result.start_line.unwrap_or(1);
                let start = start.max(first);
                let mut retained = result.raw_output.split_inclusive('\n').count();
                // A `line_truncated` tail is half a source line, so it is not a
                // complete line the reader can return. The advance rule already
                // sent the model past it, which keeps the two views consistent.
                let partial_tail = result.line_truncated
                    && !result.raw_output.is_empty()
                    && !result.raw_output.ends_with('\n');
                if partial_tail {
                    retained = retained.saturating_sub(1);
                }
                let retained_end = first.saturating_add(retained).saturating_sub(1);
                // A restored projection holds only a prefix: reporting "no lines"
                // for the rest would look like an empty range.
                if result.truncated && start > retained_end {
                    let retained_scope = if retained == 0 {
                        format!("no complete lines from {first} (only the start of that long line)")
                    } else {
                        format!("only lines {first}..{retained_end}")
                    };
                    return Err(crate::ToolError::InvalidInput(format!(
                        "receipt {id} retained {retained_scope} of {}; \
                         read the file again with a line range or locate the content with search",
                        result.total_lines.unwrap_or(retained_end)
                    )));
                }
                let take = end
                    .saturating_add(1)
                    .saturating_sub(start)
                    .min(retained_end.saturating_add(1).saturating_sub(start));
                let text = result
                    .raw_output
                    .split_inclusive('\n')
                    .skip(start.saturating_sub(first))
                    .take(take)
                    .collect();
                Ok(crate::ToolOutput::Verbatim {
                    text,
                    total_lines: result.total_lines.unwrap_or_default(),
                    start_line: start,
                    truncated: false,
                    line_truncated: false,
                })
            }
            RetainedOutput::Raw(raw) => {
                let pretty = serde_json::from_str::<Value>(raw)
                    .and_then(|v| serde_json::to_string_pretty(&v))
                    .unwrap_or_else(|_| raw.clone());
                Ok(crate::ToolOutput::Text(
                    pretty
                        .lines()
                        .enumerate()
                        .skip(start - 1)
                        .take(end - start + 1)
                        .map(|(i, line)| format!("{}: {line}", i + 1))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ))
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
// Independent persisted flags, each with its own meaning and serde default.
#[allow(clippy::struct_excessive_bools)]
pub struct ToolResult {
    /// Duplicate discovery responses reference the original receipt, without a
    /// second copy of its body. Typed same-round dependencies follow this ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reused_from: Option<String>,
    /// Source content must never be interpreted as JSON or compiler diagnostics.
    #[serde(default, skip_serializing_if = "is_false")]
    pub verbatim: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_lines: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    /// A single source line exceeded the allowance; only its beginning is kept.
    #[serde(default, skip_serializing_if = "is_false")]
    pub line_truncated: bool,
    /// Set only from a typed runtime permission failure, never inferred from prose.
    #[serde(default, skip_serializing_if = "is_false")]
    pub permission_denied: bool,
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
    /// Original content for typed data consumers; display prefixes stay in history.
    /// Only strip the exact coordinate sequence declared by a verbatim tool.
    #[must_use]
    pub fn source_text(&self) -> Cow<'_, str> {
        if !self.verbatim {
            return Cow::Borrowed(&self.raw_output);
        }
        let mut number = self.start_line.unwrap_or(1);
        let mut source = String::new();
        for line in self.raw_output.split_inclusive('\n') {
            let Some(content) = line.strip_prefix(&format!("{number}: ")) else {
                return Cow::Borrowed(&self.raw_output);
            };
            source.push_str(content);
            number = number.saturating_add(1);
        }
        Cow::Owned(source)
    }

    /// Preserve the output semantics declared by the executing tool.
    #[must_use]
    pub fn from_output(output: &crate::ToolOutput) -> Self {
        match output {
            crate::ToolOutput::Verbatim {
                text,
                total_lines,
                start_line,
                truncated,
                line_truncated,
            } => Self {
                reused_from: None,
                verbatim: true,
                total_lines: Some(*total_lines),
                start_line: Some(*start_line),
                line_truncated: *line_truncated,
                permission_denied: false,
                global_blocker: None,
                status: "success".into(),
                summary: "Operation succeeded".into(),
                diagnostics: Vec::new(),
                raw_output: text.clone(),
                truncated: *truncated,
            },
            crate::ToolOutput::Text(text)
            | crate::ToolOutput::Image {
                description: text, ..
            } => Self::new(true, text.clone()),
        }
    }

    /// Normal read projection: prefer a complete source-line prefix within the allowance.
    /// A first line longer than the allowance keeps only its beginning and sets
    /// `line_truncated`, so the next read advances past it instead of repeating
    /// the same empty line. Stored source coordinates survive repeated
    /// projection and session restore. Compression uses `compact_verbatim_lines` instead.
    #[must_use]
    pub fn compact_verbatim(&self, max_chars: usize) -> Self {
        let mut result = self.clone();
        result.line_truncated = false;
        let mut remaining = max_chars;
        let mut end = 0;
        for line in self.raw_output.split_inclusive('\n') {
            let length = line.chars().count();
            if length <= remaining {
                remaining -= length;
                end += line.len();
                continue;
            }
            if end == 0 {
                // Only the first line is eligible: keeping a UTF-8 boundary-safe
                // prefix is better than an empty body that cannot advance.
                result.line_truncated = true;
                end = line
                    .chars()
                    .take(remaining)
                    .map(char::len_utf8)
                    .sum::<usize>();
            }
            break;
        }
        result.truncated |= end < self.raw_output.len();
        result.line_truncated |= self.line_truncated && end == self.raw_output.len();
        result.raw_output.truncate(end);
        result
    }

    /// Compression keeps only complete source lines. Unlike the normal read
    /// projection, it never invents a partial first line or advances past one.
    #[must_use]
    pub fn compact_verbatim_lines(&self, max_chars: usize) -> Self {
        let mut result = self.clone();
        let mut remaining = max_chars;
        let mut end = 0;
        for line in self.raw_output.split_inclusive('\n') {
            if self.line_truncated
                && end + line.len() == self.raw_output.len()
                && !line.ends_with('\n')
            {
                break;
            }
            let length = line.chars().count();
            if length > remaining {
                break;
            }
            remaining -= length;
            end += line.len();
        }
        result.line_truncated = false;
        result.truncated |= end < self.raw_output.len();
        result.raw_output.truncate(end);
        result
    }

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
                        if success {
                            return false;
                        }
                        let lower = line.to_lowercase();
                        lower.contains("error")
                            || lower.contains("failed")
                            || lower.contains("conflict")
                    })
                    .take(12)
                    .map(|line| json!({"message":line}))
                    .collect()
            });
        Self { reused_from: None, verbatim: false, total_lines: None, start_line: None, line_truncated: false, permission_denied: false, global_blocker: None, status: if success { "success" } else { "error" }.into(),
            summary: if success { "Operation succeeded" } else { "Operation failed; repair the reported diagnostic locally and run the smallest relevant check before broader tests" }.into(),
            diagnostics, raw_output, truncated: parsed.as_ref().and_then(|v| v["truncated"].as_bool()).unwrap_or(false) }
    }

    /// Budget is supplied by `ContextBudget`, never by the tool or provider.
    #[must_use]
    pub fn model_view(&self, max_chars: usize) -> String {
        if self.verbatim {
            let result = self.compact_verbatim(max_chars);
            let start = result.start_line.unwrap_or(1);
            let displayed = result.raw_output.split_inclusive('\n').count();
            let shown = start
                .saturating_sub(1)
                .saturating_add(displayed)
                .min(result.total_lines.unwrap_or(usize::MAX));
            // A clipped single line still has to advance, or the next read would
            // repeat the same line forever.
            let partial_line = shown.max(start);
            let next = if result.line_truncated {
                result
                    .total_lines
                    .is_none_or(|total| partial_line < total)
                    .then(|| partial_line.saturating_add(1))
            } else {
                (result.truncated && result.total_lines.is_none_or(|total| shown < total))
                    .then(|| shown.saturating_add(1))
            };
            let mut view = json!({
                "verbatim": true, "output": result.raw_output,
                "total_lines": result.total_lines, "start_line": start,
                "shown_through_line": shown,
                "next_start_line": next,
                "truncated": result.truncated,
                "line_truncated": result.line_truncated,
                "raw_output_available": true
            });
            if result.line_truncated {
                view["note"] = json!(format!(
                    "line {partial_line} exceeds the read or display limit; only its beginning is shown. \
                     Use search to locate content inside this line."
                ));
            }
            return view.to_string();
        }
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
        let mut view = json!({"global_blocker":self.global_blocker,"status":self.status,"summary":self.summary,"diagnostics":diagnostics,
            "output":clipped,"truncated":self.truncated || clipped.len()<cleaned.len() || cleaned != pretty,
            "raw_output_available":true});
        if self.permission_denied {
            view["permission_denied"] = json!(true);
        }
        if let Some(first) = &self.reused_from {
            view["reused_from"] = json!(first);
        }
        view.to_string()
    }
}
#[allow(clippy::trivially_copy_pass_by_ref)] // Serde skip callback requires a reference.
fn is_false(value: &bool) -> bool {
    !value
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
    use crate::Tool;
    use std::fmt::Write as _;

    /// A first line longer than the allowance used to produce an empty body and
    /// the same `next_start_line`, so a reread could never make progress.
    #[test]
    fn oversized_first_line_keeps_a_prefix_and_advances() {
        let result = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: format!("1: {}\n2: short\n", "é".repeat(200)),
            total_lines: 2,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        let compacted = result.compact_verbatim(10);
        assert_eq!(compacted.raw_output, "1: ééééééé");
        assert!(compacted.line_truncated);
        assert!(compacted.truncated);

        let view: Value = serde_json::from_str(&result.model_view(10)).unwrap();
        assert_eq!(view["line_truncated"], true);
        assert_eq!(view["next_start_line"], 2);
        assert_eq!(view["start_line"], 1);
        assert_eq!(view["shown_through_line"], 1);
        assert_eq!(view["total_lines"], 2);
        assert!(view["note"].as_str().unwrap().contains("search"));

        // The next read starts at line 2 and completes normally.
        let next: Value = serde_json::from_str(
            &ToolResult::from_output(&crate::ToolOutput::Verbatim {
                text: "2: short\n".into(),
                total_lines: 2,
                start_line: 2,
                truncated: false,
                line_truncated: false,
            })
            .model_view(10),
        )
        .unwrap();
        assert_eq!(next["line_truncated"], false);
        assert!(next["next_start_line"].is_null());
    }

    /// A clipped line in the middle of a range must not be reported as shown, and
    /// the projection must not claim a partial line was complete.
    #[test]
    fn oversized_later_line_stops_before_it_without_line_truncation() {
        let result = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: format!("1: ab\n2: {}\n", "x".repeat(100)),
            total_lines: 2,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        let view: Value = serde_json::from_str(&result.model_view(10)).unwrap();
        assert_eq!(view["output"], "1: ab\n");
        assert_eq!(view["line_truncated"], false);
        assert_eq!(view["shown_through_line"], 1);
        assert_eq!(view["next_start_line"], 2);
        assert!(view.get("note").is_none());
    }

    /// Verbatim views carry only source fields; the removed ones carry no
    /// information for declared source text.
    #[test]
    fn verbatim_view_omits_fields_it_cannot_inform() {
        let result = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: "1: content\n".into(),
            total_lines: 1,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        let view: Value = serde_json::from_str(&result.model_view(4096)).unwrap();
        let mut keys: Vec<_> = view.as_object().unwrap().keys().cloned().collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "line_truncated",
                "next_start_line",
                "output",
                "raw_output_available",
                "shown_through_line",
                "start_line",
                "total_lines",
                "truncated",
                "verbatim"
            ]
        );
    }

    /// A zero allowance marks a partial line, but EOF has no continuation.
    #[test]
    fn zero_allowance_still_advances_past_a_long_first_line() {
        let result = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: format!("1: {}\n", "x".repeat(50)),
            total_lines: 1,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        let view: Value = serde_json::from_str(&result.model_view(0)).unwrap();
        assert_eq!(view["output"], "");
        assert_eq!(view["line_truncated"], true);
        assert!(view["next_start_line"].is_null());
    }

    /// Compact JSON must reach the model byte for byte, differing only by the
    /// declared `N: ` prefixes.
    #[test]
    fn compact_json_source_keeps_exact_bytes() {
        let body = r#"{"name":"ax","scripts":{"build":"tsc"},"private":true}"#;
        let result = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: format!("1: {body}"),
            total_lines: 1,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        let view: Value = serde_json::from_str(&result.model_view(4096)).unwrap();
        assert_eq!(view["output"], format!("1: {body}"));
        assert_eq!(result.source_text(), body);
    }

    /// A `warning:` line is source content; successful shell-style output still
    /// filters compiler warnings.
    #[test]
    fn warning_lines_survive_source_and_vanish_from_command_output() {
        let source = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: "1: warning: file content\n".into(),
            total_lines: 1,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        assert!(source.model_view(4096).contains("warning: file content"));

        let command = ToolResult::new(
            true,
            "warning: unused variable\nbuild complete\nexit_code: 0".into(),
        );
        let view: Value = serde_json::from_str(&command.model_view(4096)).unwrap();
        assert_eq!(view["status"], "success");
        assert!(!view["output"].as_str().unwrap().contains("warning:"));
        assert!(view["output"].as_str().unwrap().contains("build complete"));
    }

    /// Old saved envelopes predate the additive fields and stay ordinary text.
    #[test]
    fn legacy_envelope_without_source_fields_is_plain_text() {
        let old = r#"{"status":"success","summary":"Operation succeeded","diagnostics":[],"raw_output":"exit_code: 0\nok","truncated":false}"#;
        let result: ToolResult = serde_json::from_str(old).unwrap();
        assert!(!result.verbatim);
        assert!(!result.line_truncated);
        assert!(result.start_line.is_none());
        let view: Value = serde_json::from_str(&result.model_view(4096)).unwrap();
        assert_eq!(view["output"], "exit_code: 0\nok");
        assert_eq!(view["status"], "success");
    }

    /// `source_text` restores the original body; a non-contiguous prefix run is
    /// returned untouched rather than being partly stripped.
    #[test]
    fn source_text_round_trips_or_returns_the_original() {
        let declared = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: "7: first\n8: 9: literal\n".into(),
            total_lines: 8,
            start_line: 7,
            truncated: false,
            line_truncated: false,
        });
        assert_eq!(declared.source_text(), "first\n9: literal\n");

        let broken = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: "7: first\n9: second\n".into(),
            total_lines: 9,
            start_line: 7,
            truncated: false,
            line_truncated: false,
        });
        assert_eq!(broken.source_text(), "7: first\n9: second\n");
    }

    /// Old caches were a plain `HashMap<String, String>` of raw output.
    #[test]
    fn legacy_result_cache_deserializes_as_raw_output() {
        let cache: HashMap<String, RetainedOutput> =
            serde_json::from_str(r#"{"call-1":"exit_code: 0\nok"}"#).unwrap();
        match &cache["call-1"] {
            RetainedOutput::Raw(raw) => assert_eq!(raw, "exit_code: 0\nok"),
            RetainedOutput::Verbatim(_) => panic!("legacy string must stay raw"),
        }
        assert!(!cache["call-1"].is_partial());
    }

    /// A `line_truncated` receipt holds only the beginning of its first line and
    /// no complete line at all, so no range can be served from it; the error says
    /// why instead of returning a fragment as if it were a line.
    #[tokio::test]
    async fn line_truncated_receipt_never_returns_half_a_line() {
        let original = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: format!("1: {}\n2: second\n", "x".repeat(400)),
            total_lines: 2,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        let compacted = original.compact_verbatim(20);
        assert!(compacted.line_truncated);
        assert!(!compacted.raw_output.ends_with('\n'));
        let view: Value = serde_json::from_str(&compacted.model_view(4096)).unwrap();
        assert_eq!(view["next_start_line"], 2);

        let reader = ResultReader::default();
        reader.restore("half", &compacted);
        for start in [1, 2] {
            let error = reader
                .execute_output(json!({"call_id":"half","start_line":start,"end_line":start}))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("no complete lines"), "{error}");
            assert!(error.to_string().contains("search"), "{error}");
        }

        // A receipt that keeps a complete leading line still serves it.
        let mixed = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: format!("1: ab\n2: {}\n", "y".repeat(400)),
            total_lines: 2,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        })
        .compact_verbatim(20);
        assert!(!mixed.line_truncated);
        let reader = ResultReader::default();
        reader.restore("mixed", &mixed);
        let output = reader
            .execute_output(json!({"call_id":"mixed","start_line":1,"end_line":1}))
            .await
            .unwrap();
        assert_eq!(ToolResult::from_output(&output).raw_output, "1: ab\n");
    }

    /// A compacted envelope restored from history must not be treated as the
    /// complete receipt: an unretained range reports why instead of going empty.
    #[tokio::test]
    async fn truncated_receipt_refuses_ranges_it_no_longer_retains() {
        let original = ToolResult::from_output(&crate::ToolOutput::Verbatim {
            text: (1..=40).fold(String::new(), |mut text, n| {
                let _ = writeln!(text, "{n}: line {n}");
                text
            }),
            total_lines: 40,
            start_line: 1,
            truncated: false,
            line_truncated: false,
        });
        let reader = ResultReader::default();
        reader.restore("read", &original);
        let compacted = original.compact_verbatim(60);
        assert!(compacted.truncated);
        reader.restore("read", &compacted);
        let retained = compacted.raw_output.split_inclusive('\n').count();
        {
            let store = reader.0.read().unwrap();
            assert!(
                matches!(&store["read"], RetainedOutput::Verbatim(r) if r.raw_output.len() > compacted.raw_output.len()),
                "a compacted projection must not replace the complete receipt"
            );
        }

        // With only the truncated envelope available, the cache stays partial.
        let partial = ResultReader::default();
        partial.restore("read", &compacted);
        let error = partial
            .execute_output(json!({"call_id":"read","start_line":39,"end_line":40}))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("retained only lines"), "{message}");
        assert!(message.contains("search"), "{message}");
        assert!(retained < 40);

        let inside = partial
            .execute_output(json!({"call_id":"read","start_line":1,"end_line":2}))
            .await
            .unwrap();
        assert!(matches!(inside, crate::ToolOutput::Verbatim { .. }));
    }

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
