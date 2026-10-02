//! `request_user_input`: the one structured way for the agent to ask the user
//! how it should proceed.
//!
//! The tool answers exactly one question — "what do you want here?" — and never
//! "may I run this?". Authorization stays in the permission system
//! ([`crate::ApprovalPolicy`]); a question is a planning input, not a safety
//! gate.
//!
//! Asking suspends the run instead of ending it: the question, the goal, the
//! task queue and the session are checkpointed, the pending tool call is left
//! unanswered, and the answer is written back to that same call id so execution
//! resumes from the exact position it stopped at.

use model::{FunctionSpec, Message, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Durable marker for a pending question. Stored like the task queue: durable
/// orchestration state, independent of the conversation snapshot.
pub const STATE_PREFIX: &str = "[ax-user-question]\n";
pub const TOOL_NAME: &str = "request_user_input";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserQuestion {
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<UserOption>,
    #[serde(default)]
    pub allow_free_text: bool,
    /// Tool call this question belongs to; the answer is written back here.
    #[serde(default)]
    pub tool_call_id: String,
}

impl std::fmt::Display for UserQuestion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.question)?;
        for (index, option) in self.options.iter().enumerate() {
            write!(
                formatter,
                "\n  {}. {} ({}){}",
                index + 1,
                option.label,
                option.id,
                if option.description.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", option.description)
                }
            )?;
        }
        if self.allow_free_text {
            formatter.write_str("\n  (free text allowed)")?;
        }
        Ok(())
    }
}

impl UserQuestion {
    /// Machine-readable payload carried by the tool result that resumes the run.
    #[must_use]
    pub fn answer_payload(question: &Self, answer: &UserAnswer) -> Value {
        json!({
            "status": "answered",
            "question_id": question.id,
            "question": question.question,
            "answer": {
                "option_id": answer.option_id,
                "text": answer.text,
            }
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserAnswer {
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl UserAnswer {
    #[must_use]
    pub fn option(question: &UserQuestion, id: impl Into<String>) -> Self {
        Self {
            question_id: question.id.clone(),
            option_id: Some(id.into()),
            text: None,
        }
    }

    #[must_use]
    pub fn free_text(question: &UserQuestion, text: impl Into<String>) -> Self {
        Self {
            question_id: question.id.clone(),
            option_id: None,
            text: Some(text.into()),
        }
    }

    /// Interpret a raw UI submission: an option id, a 1-based option number or
    /// an option label selects that option; anything else is free text when the
    /// question allows it.
    #[must_use]
    pub fn parse(question: &UserQuestion, raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_lowercase();
        if let Some(option) = question
            .options
            .iter()
            .find(|option| option.id.to_lowercase() == lower)
        {
            return Some(Self::option(question, option.id.clone()));
        }
        if let Ok(index) = trimmed.parse::<usize>()
            && index >= 1
            && let Some(option) = question.options.get(index - 1)
        {
            return Some(Self::option(question, option.id.clone()));
        }
        if let Some(option) = question
            .options
            .iter()
            .find(|option| option.label.to_lowercase() == lower)
        {
            return Some(Self::option(question, option.id.clone()));
        }
        question
            .allow_free_text
            .then(|| Self::free_text(question, trimmed))
    }

    #[must_use]
    pub fn summary(&self) -> String {
        match (&self.option_id, &self.text) {
            (Some(id), _) => id.clone(),
            (None, Some(text)) => text.clone(),
            (None, None) => "(no answer)".into(),
        }
    }
}

pub(crate) fn spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: TOOL_NAME.into(),
            description: "Ask the user how to proceed when the answer materially changes the product or the final behaviour and the repository, configuration and documentation do not already answer it. Use options for discrete choices. This is not a permission request: dangerous operations are authorized by the permission system. Asking suspends this run and resumes it from the same place once the user answers. Call it alone in a round.".into(),
            parameters: json!({"type":"object","properties":{
                "question":{"type":"string","description":"One concrete question, in the user's language"},
                "options":{"type":"array","items":{"type":"object","properties":{
                    "id":{"type":"string","description":"Stable identifier returned as the answer"},
                    "label":{"type":"string"},
                    "description":{"type":"string"}
                },"required":["id","label"],"additionalProperties":false}},
                "allow_free_text":{"type":"boolean","description":"Whether an answer outside the options is acceptable"}
            },"required":["question"]}),
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
/// Rejects a question without text or with duplicate option identifiers.
pub(crate) fn parse_question(input: &Value, tool_call_id: &str) -> Result<UserQuestion, String> {
    let question = input["question"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or("question text is required")?;
    let mut options = Vec::new();
    if let Some(raw) = input.get("options") {
        let raw = raw.as_array().ok_or("options must be an array")?;
        for (index, value) in raw.iter().enumerate() {
            let id = value["id"]
                .as_str()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| format!("option {} requires an id", index + 1))?;
            if options.iter().any(|option: &UserOption| option.id == id) {
                return Err(format!("duplicate option id `{id}`"));
            }
            let label = value["label"]
                .as_str()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or(id);
            options.push(UserOption {
                id: id.to_owned(),
                label: label.to_owned(),
                description: value["description"].as_str().unwrap_or_default().to_owned(),
            });
        }
    }
    let allow_free_text = input["allow_free_text"]
        .as_bool()
        .unwrap_or(options.is_empty());
    Ok(UserQuestion {
        id: format!("q-{tool_call_id}"),
        question: question.to_owned(),
        options,
        allow_free_text,
        tool_call_id: tool_call_id.to_owned(),
    })
}

#[must_use]
pub(crate) fn snapshot(question: &UserQuestion) -> Message {
    Message::system(format!(
        "{STATE_PREFIX}{}",
        serde_json::to_string(question).unwrap_or_default()
    ))
}

/// Latest durable question, with every question marker removed from the vector
/// so a resumed or forked kernel never replays a stale one.
#[must_use]
pub(crate) fn restore(messages: &mut Vec<Message>) -> Option<UserQuestion> {
    let question = messages
        .iter()
        .rev()
        .filter(|message| message.role == model::Role::System)
        .find_map(|message| {
            message
                .content
                .strip_prefix(STATE_PREFIX)
                .and_then(|json| serde_json::from_str(json).ok())
        });
    messages.retain(|message| {
        message.role != model::Role::System || !message.content.starts_with(STATE_PREFIX)
    });
    question
}

#[cfg(test)]
#[path = "user_input_tests.rs"]
mod tests;
