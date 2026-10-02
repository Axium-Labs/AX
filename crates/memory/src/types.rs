//! The values this crate stores and returns.
//!
//! Pure data: no connection, no I/O. `MemoryStore` is the only thing that
//! turns these into rows.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::MemoryError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    System,
}

impl MessageRole {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::System => "system",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, MemoryError> {
        match value {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            "system" => Ok(Self::System),
            other => Err(MemoryError::InvalidValue(format!(
                "unknown message role: {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Message,
    ToolCall,
    McpCall,
    AgentState,
    Summary,
}

impl MessageKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::ToolCall => "tool_call",
            Self::McpCall => "mcp_call",
            Self::AgentState => "agent_state",
            Self::Summary => "summary",
        }
    }

    pub(crate) fn from_db(value: &str) -> Result<Self, MemoryError> {
        match value {
            "message" => Ok(Self::Message),
            "tool_call" => Ok(Self::ToolCall),
            "mcp_call" => Ok(Self::McpCall),
            "agent_state" => Ok(Self::AgentState),
            "summary" => Ok(Self::Summary),
            other => Err(MemoryError::InvalidValue(format!(
                "unknown message kind: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewMessage {
    pub role: MessageRole,
    pub kind: MessageKind,
    pub content: String,
    pub metadata: Value,
}

impl NewMessage {
    #[must_use]
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            kind: MessageKind::Message,
            content: content.into(),
            metadata: Value::Null,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    pub role: MessageRole,
    pub kind: MessageKind,
    pub content: String,
    pub metadata: Value,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LongTermMemory {
    pub id: String,
    pub key: String,
    pub value: String,
    pub category: String,
    pub created_at: i64,
    pub updated_at: i64,
}
