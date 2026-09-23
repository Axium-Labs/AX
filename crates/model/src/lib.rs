//! Provider-neutral model contracts and provider implementations.

mod auth;
mod codex_device;
mod openai;
mod openai_compatible;
mod providers;
mod registry;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub use openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
pub use providers::DEEPSEEK_FALLBACK_MODEL;
pub use providers::deepseek_compatible_config;
#[deprecated(note = "use OpenAiCompatibleConfig")]
pub type DeepSeekConfig = OpenAiCompatibleConfig;
#[deprecated(note = "use OpenAiCompatibleProvider")]
pub type DeepSeekProvider = OpenAiCompatibleProvider;
pub use openai::{FALLBACK_MODEL as OPENAI_FALLBACK_MODEL, OpenAiConfig, OpenAiProvider};
pub use providers::{
    PROVIDERS, ProviderAuthKind, ProviderProtocol, ProviderSpec, provider, provider_base_url,
    provider_chat_endpoint, provider_supports_oauth,
};
pub use registry::{CatalogSource, ModelCatalog, ModelRegistry};

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("model configuration error: {0}")]
    Configuration(String),
    #[error("model transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("model returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("model HTTP request failed with status {status}: {message}")]
    HttpStatus { status: u16, message: String },
    #[error("model catalog I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl std::fmt::Display for ReasoningEffort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        };
        formatter.write_str(value)
    }
}

impl ReasoningEffort {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "low" | "minimal" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" | "ultra" => Some(Self::Max),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub context_window: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<usize>,
    pub reasoning_efforts: Vec<ReasoningEffort>,
    pub default_reasoning_effort: Option<ReasoningEffort>,
    pub supports_tools: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<ContentPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl Message {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            parts: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            parts: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            parts: Vec::new(),
            tool_call_id: None,
            tool_calls,
        }
    }

    #[must_use]
    pub fn tool(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            parts: Vec::new(),
            tool_call_id: Some(call_id.into()),
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { media_type: String, data: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub vision: bool,
    pub tool_calling: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// OpenAI-compatible APIs encode arguments as a JSON string.
    pub arguments: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionSpec,
}

#[derive(Clone, Debug, Serialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Debug)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Clone, Debug)]
pub struct ModelResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn model_id(&self) -> &str;
    fn context_window(&self) -> usize;
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            vision: false,
            tool_calling: true,
        }
    }
    fn max_output_tokens(&self) -> Option<usize> {
        None
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        Ok(self.fallback_models())
    }

    fn fallback_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: self.model_id().to_owned(),
            display_name: self.model_id().to_owned(),
            provider: self.name().to_owned(),
            context_window: self.context_window(),
            max_output_tokens: self.max_output_tokens(),
            reasoning_efforts: Vec::new(),
            default_reasoning_effort: None,
            supports_tools: true,
            endpoint: None,
        }]
    }

    async fn complete_stream(
        &self,
        request: ModelRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
        on_thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        let _ = on_thinking;
        let response = self.complete(request).await?;
        if !response.content.is_empty() {
            on_delta(response.content.clone());
        }
        Ok(response)
    }
}
pub use auth::{AuthStorage, OAuthCredential};
pub use codex_device::{DeviceAuth, DeviceTokens, account_id_from_token, begin, refresh_oauth};

#[cfg(test)]
mod multimodal_tests {
    use super::*;
    #[test]
    fn image_parts_survive_session_metadata_round_trip() {
        let mut message = Message::tool("call", "image.png");
        message.parts = vec![ContentPart::Image {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        }];
        let restored: Message =
            serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
        assert_eq!(restored.parts, message.parts);
        assert_eq!(restored.content, "image.png");
    }
}
