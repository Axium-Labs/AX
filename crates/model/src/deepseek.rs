use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::{
    FunctionCall, Message, ModelError, ModelInfo, ModelProvider, ModelRequest, ModelResponse,
    ReasoningEffort, ToolCall, ToolSpec,
};

const DEFAULT_ENDPOINT: &str = "https://api.deepseek.com/chat/completions";
pub const FALLBACK_MODEL: &str = "deepseek-flash";

#[derive(Clone, Debug)]
pub struct DeepSeekConfig {
    pub provider_id: String,
    pub api_key: String,
    pub model: String,
    pub endpoint: String,
    pub context_window: usize,
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl DeepSeekConfig {
    #[must_use]
    pub fn from_api_key(model: Option<String>, api_key: String) -> Self {
        Self {
            provider_id: "deepseek".to_owned(),
            api_key,
            model: model.unwrap_or_else(|| FALLBACK_MODEL.to_owned()),
            endpoint: std::env::var("DEEPSEEK_API_URL")
                .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned()),
            context_window: 1_048_576,
            reasoning_effort: None,
        }
    }

    /// Builds a provider configuration from the `DeepSeek` environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::Configuration`] when `DEEPSEEK_API_KEY` is absent.
    pub fn from_env(model: Option<String>) -> Result<Self, ModelError> {
        let api_key = std::env::var("DEEPSEEK_API_KEY")
            .map_err(|_| ModelError::Configuration("DEEPSEEK_API_KEY is not set".to_owned()))?;
        let endpoint =
            std::env::var("DEEPSEEK_API_URL").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        Ok(Self {
            provider_id: "deepseek".to_owned(),
            api_key,
            model: model.unwrap_or_else(|| FALLBACK_MODEL.to_owned()),
            endpoint,
            context_window: 1_048_576,
            reasoning_effort: None,
        })
    }

    #[must_use]
    pub fn from_compatible(
        provider_id: impl Into<String>,
        model: String,
        api_key: String,
        endpoint: String,
        context_window: usize,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            api_key,
            model,
            endpoint,
            context_window,
            reasoning_effort: None,
        }
    }
}

pub struct DeepSeekProvider {
    client: reqwest::Client,
    config: DeepSeekConfig,
}

impl DeepSeekProvider {
    #[must_use]
    pub fn new(config: DeepSeekConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    tools: &'a [ToolSpec],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<RemoteModel>,
}

#[derive(Deserialize)]
struct RemoteModel {
    id: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    content: Option<String>,
    /// `DeepSeek` reasoning models stream their chain-of-thought here.
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<DeltaToolCall>,
}

#[derive(Deserialize)]
struct DeltaToolCall {
    index: usize,
    id: Option<String>,
    function: Option<DeltaFunction>,
}

#[derive(Deserialize)]
struct DeltaFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[async_trait]
impl ModelProvider for DeepSeekProvider {
    fn name(&self) -> &str {
        &self.config.provider_id
    }

    fn model_id(&self) -> &str {
        &self.config.model
    }

    fn context_window(&self) -> usize {
        self.config.context_window
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let response = self
            .client
            .post(&self.config.endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&ChatRequest {
                model: &self.config.model,
                messages: &request.messages,
                tools: &request.tools,
                stream: false,
                reasoning_effort: self.config.reasoning_effort,
            })
            .send()
            .await?
            .error_for_status()?
            .json::<ChatResponse>()
            .await?;

        let choice = response.choices.into_iter().next().ok_or_else(|| {
            ModelError::InvalidResponse("response contains no choices".to_owned())
        })?;
        Ok(ModelResponse {
            content: choice.message.content.unwrap_or_default(),
            tool_calls: choice.message.tool_calls,
            finish_reason: choice.finish_reason,
        })
    }

    async fn complete_stream(
        &self,
        request: ModelRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
        on_thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        let response = self
            .client
            .post(&self.config.endpoint)
            .bearer_auth(&self.config.api_key)
            .json(&ChatRequest {
                model: &self.config.model,
                messages: &request.messages,
                tools: &request.tools,
                stream: true,
                reasoning_effort: self.config.reasoning_effort,
            })
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut content = String::new();
        let mut tool_calls = BTreeMap::<usize, PartialToolCall>::new();
        let mut finish_reason = None;

        while let Some(chunk) = stream.next().await {
            buffer.extend_from_slice(&chunk?);
            while let Some((end, delimiter_len)) = find_event_end(&buffer) {
                let event = buffer.drain(..end).collect::<Vec<_>>();
                buffer.drain(..delimiter_len);
                process_event(
                    &event,
                    &mut content,
                    &mut tool_calls,
                    &mut finish_reason,
                    on_delta,
                    on_thinking,
                )?;
            }
        }
        if !buffer.is_empty() {
            process_event(
                &buffer,
                &mut content,
                &mut tool_calls,
                &mut finish_reason,
                on_delta,
                on_thinking,
            )?;
        }
        Ok(ModelResponse {
            content,
            tool_calls: tool_calls
                .into_values()
                .map(|call| ToolCall {
                    id: call.id,
                    kind: "function".to_owned(),
                    function: FunctionCall {
                        name: call.name,
                        arguments: call.arguments,
                    },
                })
                .collect(),
            finish_reason,
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let endpoint = self
            .config
            .endpoint
            .trim_end_matches("/chat/completions")
            .trim_end_matches('/')
            .to_owned()
            + "/models";
        let response = self
            .client
            .get(endpoint)
            .timeout(Duration::from_secs(10))
            .bearer_auth(&self.config.api_key)
            .send()
            .await?
            .error_for_status()?
            .json::<ModelsResponse>()
            .await?;
        Ok(response
            .data
            .into_iter()
            .map(|model| {
                compatible_model_info(model.id, &self.config.provider_id, &self.config.endpoint)
            })
            .collect())
    }

    fn fallback_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }
}

fn compatible_model_info(id: String, provider: &str, endpoint: &str) -> ModelInfo {
    let deepseek = provider == "deepseek";
    let reasoning = deepseek && id.to_ascii_lowercase().contains("reason");
    ModelInfo {
        display_name: id.clone(),
        id,
        provider: provider.to_owned(),
        context_window: if deepseek { 1_048_576 } else { 128_000 },
        reasoning_efforts: if reasoning {
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]
        } else {
            Vec::new()
        },
        default_reasoning_effort: reasoning.then_some(ReasoningEffort::High),
        supports_tools: true,
        endpoint: (!deepseek).then(|| endpoint.to_owned()),
    }
}

fn find_event_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| (position, 2));
    let crlf = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4));
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(found), None) | (None, Some(found)) => Some(found),
        (None, None) => None,
    }
}

fn process_event(
    event: &[u8],
    content: &mut String,
    tool_calls: &mut BTreeMap<usize, PartialToolCall>,
    finish_reason: &mut Option<String>,
    on_delta: &mut (dyn FnMut(String) + Send),
    on_thinking: &mut (dyn FnMut(String) + Send),
) -> Result<(), ModelError> {
    let event = std::str::from_utf8(event)
        .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
    let Some(data) = event
        .lines()
        .find_map(|line| line.trim_end_matches('\r').strip_prefix("data:"))
        .map(str::trim)
    else {
        return Ok(());
    };
    if data == "[DONE]" || data.is_empty() {
        return Ok(());
    }
    let chunk = serde_json::from_str::<StreamChunk>(data)
        .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
    for choice in chunk.choices {
        if let Some(reasoning) = choice.delta.reasoning_content {
            on_thinking(reasoning.clone());
        }
        if let Some(delta) = choice.delta.content {
            on_delta(delta.clone());
            content.push_str(&delta);
        }
        for delta in choice.delta.tool_calls {
            let call = tool_calls.entry(delta.index).or_default();
            if let Some(id) = delta.id {
                call.id.push_str(&id);
            }
            if let Some(function) = delta.function {
                if let Some(name) = function.name {
                    call.name.push_str(&name);
                }
                if let Some(arguments) = function.arguments {
                    call.arguments.push_str(&arguments);
                }
            }
        }
        if choice.finish_reason.is_some() {
            *finish_reason = choice.finish_reason;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combines_streamed_text_and_tool_arguments() {
        let mut content = String::new();
        let mut calls = BTreeMap::new();
        let mut finish = None;
        let mut deltas = Vec::new();
        let mut thinking = Vec::new();
        let mut on_delta = |delta: String| deltas.push(delta);
        let mut on_thinking = |delta: String| thinking.push(delta);
        process_event(
            br#"data: {"choices":[{"delta":{"reasoning_content":"think hard","content":"hi","tool_calls":[{"index":0,"id":"call-1","function":{"name":"shell","arguments":"{\\\"com"}}]},"finish_reason":null}]}"#,
            &mut content,
            &mut calls,
            &mut finish,
            &mut on_delta,
            &mut on_thinking,
        )
        .expect("first event should parse");
        process_event(
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mand\\\":\\\"pwd\\\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut content,
            &mut calls,
            &mut finish,
            &mut on_delta,
            &mut on_thinking,
        )
        .expect("second event should parse");

        assert_eq!(content, "hi");
        assert_eq!(deltas, ["hi"]);
        assert_eq!(thinking, ["think hard"]);
        assert_eq!(calls[&0].name, "shell");
        assert_eq!(finish.as_deref(), Some("tool_calls"));
    }
}
