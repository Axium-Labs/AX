use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    FunctionCall, Message, ModelError, ModelInfo, ModelProvider, ModelRequest, ModelResponse,
    ReasoningEffort, ToolCall, ToolSpec, stats,
};

#[derive(Clone, Debug)]
pub struct OpenAiCompatibleConfig {
    pub provider_id: String,
    pub api_key: String,
    pub api_key_header: String,
    pub model: String,
    pub endpoint: String,
    pub context_window: usize,
    pub max_output_tokens: Option<usize>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl OpenAiCompatibleConfig {
    #[must_use]
    pub fn new(
        provider_id: impl Into<String>,
        model: String,
        api_key: String,
        endpoint: String,
        context_window: usize,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            api_key,
            api_key_header: "authorization".into(),
            model,
            endpoint,
            context_window,
            max_output_tokens: None,
            reasoning_effort: None,
        }
    }
}

pub struct OpenAiCompatibleProvider {
    client: reqwest::Client,
    config: OpenAiCompatibleConfig,
}

impl OpenAiCompatibleProvider {
    #[must_use]
    pub fn with_client(config: OpenAiCompatibleConfig, client: reqwest::Client) -> Self {
        Self { client, config }
    }

    #[must_use]
    pub fn new(config: OpenAiCompatibleConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    async fn complete_inner(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let response = ensure_success(
            self.client
                .post(&self.config.endpoint)
                .header(
                    &self.config.api_key_header,
                    format!("Bearer {}", self.config.api_key),
                )
                .json(&ChatRequest {
                    model: &self.config.model,
                    messages: chat_messages(&request.messages),
                    tools: &request.tools,
                    stream: false,
                    stream_options: None,
                    reasoning_effort: self.config.reasoning_effort,
                })
                .send()
                .await?,
        )
        .await?
        .json::<ChatResponse>()
        .await?;

        let choice = response.choices.into_iter().next().ok_or_else(|| {
            ModelError::InvalidResponse("response contains no choices".to_owned())
        })?;
        Ok(ModelResponse {
            provider_metadata: None,
            usage: response.usage,
            content: choice.message.content.unwrap_or_default(),
            tool_calls: choice.message.tool_calls,
            finish_reason: choice.finish_reason,
        })
    }

    /// Opens a streaming request, adapting the request shape when a provider
    /// rejects the optional `stream_options` field.
    ///
    /// This is a one-shot request-shape downgrade, not an error retry: it changes
    /// only an optional field and never repeats a request that already streamed.
    /// Retry classification stays in `model::retry` and is unaffected.
    async fn open_stream(&self, request: &ModelRequest) -> Result<reqwest::Response, ModelError> {
        let send = |include_usage: bool| {
            self.client
                .post(&self.config.endpoint)
                .header(
                    &self.config.api_key_header,
                    format!("Bearer {}", self.config.api_key),
                )
                .json(&ChatRequest {
                    model: &self.config.model,
                    messages: chat_messages(&request.messages),
                    tools: &request.tools,
                    stream: true,
                    stream_options: include_usage.then(|| json!({"include_usage":true})),
                    reasoning_effort: self.config.reasoning_effort,
                })
                .send()
        };
        match ensure_success(send(true).await?).await {
            // Some compatible providers reject this optional OpenAI field.
            Err(ModelError::HttpResponse {
                retry_after,
                status: 400 | 422,
                message,
            }) if message.contains("stream_options") || message.contains("include_usage") => {
                ensure_success(send(false).await?).await
            }
            other => other,
        }
    }

    /// Records time-to-first-delta once, for text or tool output alike.
    fn note_first_delta(
        &self,
        first_delta: &AtomicBool,
        content: &str,
        tool_calls: &BTreeMap<usize, PartialToolCall>,
        started: Instant,
    ) {
        if !first_delta.load(Ordering::SeqCst) && (!content.is_empty() || !tool_calls.is_empty()) {
            first_delta.store(true, Ordering::SeqCst);
            stats::record_ttft(
                &self.config.provider_id,
                &self.config.model,
                started.elapsed(),
            );
        }
    }

    async fn stream_inner(
        &self,
        request: ModelRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
        on_thinking: &mut (dyn FnMut(String) + Send),
        started: Instant,
    ) -> Result<ModelResponse, ModelError> {
        let response = self.open_stream(&request).await?;
        // TTFT = time to the first valid delta of any kind: reasoning, text
        // or tool call. Reasoning flows through the callback; text and tool
        // deltas are checked after each event.
        let first_delta = AtomicBool::new(false);
        let mut on_thinking = |text: String| {
            if !text.is_empty() && !first_delta.swap(true, Ordering::SeqCst) {
                stats::record_ttft(
                    &self.config.provider_id,
                    &self.config.model,
                    started.elapsed(),
                );
            }
            on_thinking(text);
        };
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut content = String::new();
        let mut tool_calls = BTreeMap::<usize, PartialToolCall>::new();
        let mut finish_reason = None;
        let mut usage = None;

        'events: while let Some(chunk) = stream.next().await {
            buffer.extend_from_slice(&chunk?);
            while let Some((end, delimiter_len)) = find_event_end(&buffer) {
                let event = buffer.drain(..end).collect::<Vec<_>>();
                buffer.drain(..delimiter_len);
                if process_event(
                    &event,
                    &mut content,
                    &mut tool_calls,
                    &mut finish_reason,
                    &mut usage,
                    on_delta,
                    &mut on_thinking,
                )? {
                    buffer.clear();
                    break 'events;
                }
                self.note_first_delta(&first_delta, &content, &tool_calls, started);
            }
        }
        if !buffer.is_empty() {
            process_event(
                &buffer,
                &mut content,
                &mut tool_calls,
                &mut finish_reason,
                &mut usage,
                on_delta,
                &mut on_thinking,
            )?;
            self.note_first_delta(&first_delta, &content, &tool_calls, started);
        }
        Ok(ModelResponse {
            provider_metadata: None,
            usage,
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
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Value>,
    tools: &'a [ToolSpec],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
}

fn chat_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let mut value = json!({"role":message.role,"content":message.content});
            if let Some(id) = &message.tool_call_id {
                value["tool_call_id"] = json!(id);
            }
            if !message.tool_calls.is_empty() {
                value["tool_calls"] = json!(message.tool_calls);
            }
            value
        })
        .collect()
}

#[cfg(test)]
mod content_tests {
    use super::*;
    #[test]
    fn final_empty_choices_chunk_preserves_usage() {
        let mut usage = None;
        process_event(
            br#"data: {"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":5}}"#,
            &mut String::new(),
            &mut BTreeMap::new(),
            &mut None,
            &mut usage,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(usage.unwrap()["completion_tokens"], 5);
    }
    #[test]
    fn text_only_provider_omits_multimodal_metadata() {
        let mut message = Message::tool("call", "image description");
        message.parts = vec![crate::ContentPart::Image {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        }];
        let output = chat_messages(&[message]);
        assert!(output[0].get("parts").is_none());
        assert_eq!(output[0]["content"], "image description");
    }
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
    #[serde(default)]
    usage: Option<Value>,
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
    usage: Option<Value>,
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
    /// Compatible reasoning models may stream reasoning content here.
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
impl ModelProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.config.provider_id
    }

    fn model_id(&self) -> &str {
        &self.config.model
    }

    fn context_window(&self) -> usize {
        self.config.context_window
    }

    fn max_output_tokens(&self) -> Option<usize> {
        self.config.max_output_tokens
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let result = self.complete_inner(request).await;
        if result.is_err() {
            stats::record_error(&self.config.provider_id, &self.config.model);
        }
        result
    }

    async fn complete_stream(
        &self,
        request: ModelRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
        on_thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        let started = Instant::now();
        let result = self
            .stream_inner(request, on_delta, on_thinking, started)
            .await;
        match &result {
            Ok(response) => {
                let output_chars = response.content.len()
                    + response
                        .tool_calls
                        .iter()
                        .map(|call| {
                            call.id.len() + call.function.name.len() + call.function.arguments.len()
                        })
                        .sum::<usize>();
                stats::record_completion(
                    &self.config.provider_id,
                    &self.config.model,
                    (output_chars / 4) as u64,
                    started.elapsed(),
                );
            }
            Err(_) => stats::record_error(&self.config.provider_id, &self.config.model),
        }
        result
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let endpoint = self
            .config
            .endpoint
            .trim_end_matches("/chat/completions")
            .trim_end_matches('/')
            .to_owned()
            + "/models";
        let response = ensure_success(
            self.client
                .get(endpoint)
                .timeout(Duration::from_secs(10))
                .header(
                    &self.config.api_key_header,
                    format!("Bearer {}", self.config.api_key),
                )
                .send()
                .await?,
        )
        .await?
        .json::<ModelsResponse>()
        .await?;
        Ok(response
            .data
            .into_iter()
            .map(|model| {
                crate::providers::compatible_model_info(
                    model.id,
                    &self.config.provider_id,
                    &self.config.endpoint,
                )
            })
            .collect())
    }

    fn fallback_models(&self) -> Vec<ModelInfo> {
        crate::builtin_models(&self.config.provider_id)
    }
}

/// Replaces `error_for_status`, which throws the response body away.
///
/// A vendor's own message is the only thing that separates "invalid key" from
/// "insufficient balance" or "model not found", and it is what the CLI, the
/// TUI and Crew show the user, so it has to survive the error path.
async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response, ModelError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let retry_after = crate::retry::retry_after_header(response.headers());
    let body = response.text().await.unwrap_or_default();
    Err(ModelError::HttpResponse {
        retry_after,
        status: status.as_u16(),
        message: summarize_error_body(&body),
    })
}

fn summarize_error_body(body: &str) -> String {
    const LIMIT: usize = 400;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "response had no body".to_owned();
    }
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_owned();
    }
    let mut summary: String = trimmed.chars().take(LIMIT).collect();
    summary.push('…');
    summary
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
    usage: &mut Option<Value>,
    on_delta: &mut (dyn FnMut(String) + Send),
    on_thinking: &mut (dyn FnMut(String) + Send),
) -> Result<bool, ModelError> {
    let event = std::str::from_utf8(event)
        .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
    let Some(data) = event
        .lines()
        .find_map(|line| line.trim_end_matches('\r').strip_prefix("data:"))
        .map(str::trim)
    else {
        return Ok(false);
    };
    let mut data = data;
    while let Some(rest) = data.strip_prefix("data:") {
        data = rest.trim();
    }
    if data == "[DONE]" {
        return Ok(true);
    }
    if data.is_empty() || data.starts_with(':') {
        return Ok(false);
    }
    let chunk = serde_json::from_str::<StreamChunk>(data)
        .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
    if chunk.usage.is_some() {
        *usage = chunk.usage;
    }
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
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `error_for_status` used to discard the body, so a vendor's own
    /// explanation never reached the user. Keep it, and keep it bounded.
    #[test]
    fn vendor_error_bodies_survive_and_stay_bounded() {
        assert_eq!(summarize_error_body("   "), "response had no body");
        assert_eq!(
            summarize_error_body(r#"{"error":{"message":"insufficient balance"}}"#),
            r#"{"error":{"message":"insufficient balance"}}"#
        );

        let long = "x".repeat(1_000);
        let summary = summarize_error_body(&long);
        assert_eq!(
            summary.chars().count(),
            401,
            "should cap at 400 plus ellipsis"
        );
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn combines_streamed_text_and_tool_arguments() {
        let mut content = String::new();
        let mut calls = BTreeMap::new();
        let mut finish = None;
        let mut usage = None;
        let mut deltas = Vec::new();
        let mut thinking = Vec::new();
        let mut on_delta = |delta: String| deltas.push(delta);
        let mut on_thinking = |delta: String| thinking.push(delta);
        process_event(
            br#"data: {"choices":[{"delta":{"reasoning_content":"think hard","content":"hi","tool_calls":[{"index":0,"id":"call-1","function":{"name":"shell","arguments":"{\\\"com"}}]},"finish_reason":null}]}"#,
            &mut content,
            &mut calls,
            &mut finish,
            &mut usage,
            &mut on_delta,
            &mut on_thinking,
        )
        .expect("first event should parse");
        process_event(
            br#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mand\\\":\\\"pwd\\\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut content,
            &mut calls,
            &mut finish,
            &mut usage,
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
