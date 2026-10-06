//! Advisory loop hygiene: detect consecutive identical tool calls and nudge the
//! model without hard-blocking.
//!
//! This is deliberately *not* a low global tool-call cap. Complex tasks
//! legitimately make many calls. What matters is repetition without progress:
//! the same tool with the same arguments, over and over. At configured
//! thresholds the runtime injects one internal system reminder telling the
//! model to inspect the previous result, change approach, or finish. The call
//! is still executed; the reminder only enriches context.

use serde_json::Value;

/// First threshold: the gentle reminder.
pub const GENTLE_THRESHOLD: usize = 3;
/// Later thresholds: the detailed reminder naming the tool and arguments.
pub const DETAILED_THRESHOLDS: &[usize] = &[5, 8];

/// The reminder to inject, if this call hit a threshold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reminder {
    /// The first threshold: a short nudge.
    Gentle,
    /// A later threshold: names the tool, run length and canonical arguments.
    Detailed { count: usize, arguments: String },
}

impl Reminder {
    /// Model-facing text for this reminder.
    #[must_use]
    pub fn text(&self, tool: &str) -> String {
        match self {
            Self::Gentle => "You are repeating the exact same tool call with identical arguments. \
                Carefully analyze the previous result before calling again: if the task is not \
                complete, try a different approach or different arguments instead of repeating \
                the call."
                .to_owned(),
            Self::Detailed { count, arguments } => format!(
                "Repeated tool call detected:\n- tool: {tool}\n- consecutive_calls: {count}\n\
                 - arguments: {arguments}\nThe repeated calls are not making progress. Do not call \
                 this tool with these exact arguments again. Inspect the latest result and choose a \
                 different action, different arguments, or finish the task if enough evidence has \
                 been gathered."
            ),
        }
    }
}

/// One agent's consecutive-repeat chain: the last call's identity and its run
/// length. A different call resets the chain; an untracked (control) call is
/// handled by the caller not observing it at all.
#[derive(Debug, Default)]
pub(crate) struct RepeatCallChain {
    key: Option<String>,
    count: usize,
}

impl RepeatCallChain {
    /// Observe one executed tool call and return the reminder it triggers, if any.
    pub(crate) fn observe(&mut self, tool: &str, arguments: &str) -> Option<Reminder> {
        let key = call_key(tool, arguments);
        self.count = if self.key.as_deref() == Some(key.as_str()) {
            self.count + 1
        } else {
            1
        };
        self.key = Some(key);
        if self.count == GENTLE_THRESHOLD {
            return Some(Reminder::Gentle);
        }
        if DETAILED_THRESHOLDS.contains(&self.count) {
            return Some(Reminder::Detailed {
                count: self.count,
                arguments: preview(&canonical(arguments), 500),
            });
        }
        None
    }

    /// Forget the chain, for example at the start of a new user turn.
    pub(crate) fn reset(&mut self) {
        self.key = None;
        self.count = 0;
    }
}

/// Canonical string form of a call's arguments: parse, deep key-sort, stringify.
/// A malformed payload falls back to its raw text so two identical raw strings
/// still compare equal.
fn canonical(arguments: &str) -> String {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value) => sort_json(value).to_string(),
        Err(_) => arguments.to_owned(),
    }
}

fn sort_json(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(sort_json).collect()),
        Value::Object(object) => {
            let mut keys: Vec<_> = object.keys().cloned().collect();
            keys.sort();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                let value = object.get(&key).cloned().unwrap_or(Value::Null);
                sorted.insert(key, sort_json(value));
            }
            Value::Object(sorted)
        }
        other => other,
    }
}

fn call_key(tool: &str, arguments: &str) -> String {
    format!("{tool}\u{1f}{}", canonical(arguments))
}

fn preview(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        return text.to_owned();
    }
    let head: String = text.chars().take(cap).collect();
    format!("{head}… (+{} more chars)", text.chars().count() - cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_identical_calls_escalate_then_reset_on_change() {
        let mut chain = RepeatCallChain::default();
        let args = r#"{"path":"a","line":1}"#;
        assert_eq!(chain.observe("filesystem", args), None);
        assert_eq!(chain.observe("filesystem", args), None);
        assert_eq!(chain.observe("filesystem", args), Some(Reminder::Gentle));
        assert_eq!(chain.observe("filesystem", args), None);
        assert!(matches!(
            chain.observe("filesystem", args),
            Some(Reminder::Detailed { count: 5, .. })
        ));
        // A different call resets the chain.
        assert_eq!(chain.observe("filesystem", r#"{"path":"b"}"#), None);
        assert_eq!(chain.observe("filesystem", r#"{"path":"b"}"#), None);
        assert_eq!(
            chain.observe("filesystem", r#"{"path":"b"}"#),
            Some(Reminder::Gentle)
        );
        chain.reset();
        assert_eq!(chain.observe("filesystem", r#"{"path":"b"}"#), None);
    }

    #[test]
    fn argument_order_does_not_break_identity() {
        let mut chain = RepeatCallChain::default();
        let a = r#"{"x":1,"y":2}"#;
        let b = r#"{"y":2,"x":1}"#;
        assert_eq!(chain.observe("t", a), None);
        assert_eq!(chain.observe("t", b), None);
        assert_eq!(chain.observe("t", a), Some(Reminder::Gentle));
    }

    #[test]
    fn malformed_arguments_still_compare_by_raw_text() {
        let mut chain = RepeatCallChain::default();
        assert_eq!(chain.observe("t", "not json"), None);
        assert_eq!(chain.observe("t", "not json"), None);
        assert_eq!(chain.observe("t", "not json"), Some(Reminder::Gentle));
    }
}
