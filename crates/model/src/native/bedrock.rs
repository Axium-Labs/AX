use super::{NativeProvider, arguments, call, string, transport};
use crate::ModelProvider;
use crate::{ContentPart, ModelError, ModelRequest, ModelResponse, Role};
use aws_credential_types::provider::ProvideCredentials;
use aws_sigv4::{
    http_request::{SignableBody, SignableRequest, SigningSettings, sign},
    sign::v4,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::SystemTime};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}
pub(crate) fn configured() -> bool {
    if [
        "AWS_BEARER_TOKEN_BEDROCK",
        "AWS_PROFILE",
        "AWS_ACCESS_KEY_ID",
        "AWS_WEB_IDENTITY_TOKEN_FILE",
        "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
        "AWS_CONTAINER_CREDENTIALS_FULL_URI",
        "AWS_SHARED_CREDENTIALS_FILE",
    ]
    .iter()
    .any(|name| env(name).is_some())
    {
        return true;
    }
    env("USERPROFILE")
        .or_else(|| env("HOME"))
        .is_some_and(|home| {
            std::path::Path::new(&home)
                .join(".aws/credentials")
                .is_file()
                || std::path::Path::new(&home).join(".aws/config").is_file()
        })
}

async fn config(provider: &NativeProvider) -> &aws_config::SdkConfig {
    provider
        .aws
        .get_or_init(|| async {
            let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
            if let Some(region) = provider
                .config
                .region
                .clone()
                .or_else(|| env("AWS_REGION"))
                .or_else(|| env("AWS_DEFAULT_REGION"))
            {
                loader = loader.region(aws_config::Region::new(region));
            }
            if let Some(credentials) = &provider.config.aws_credentials {
                loader = loader.credentials_provider(credentials.clone());
            }
            loader.load().await
        })
        .await
}

async fn authenticated(
    provider: &NativeProvider,
    url: &str,
    bytes: Vec<u8>,
) -> Result<reqwest::RequestBuilder, ModelError> {
    let request = provider
        .client
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/vnd.amazon.eventstream");
    if let Some(token) = provider
        .config
        .api_key
        .clone()
        .or_else(|| env("AWS_BEARER_TOKEN_BEDROCK"))
    {
        return Ok(request.bearer_auth(token).body(bytes));
    }
    let config = config(provider).await;
    let region = config.region().ok_or_else(|| {
        ModelError::Configuration(
            "Bedrock requires AWS_REGION, AWS_DEFAULT_REGION, or a profile region".into(),
        )
    })?;
    let credentials = config
        .credentials_provider()
        .ok_or_else(|| ModelError::Configuration("Bedrock has no AWS credential provider".into()))?
        .provide_credentials()
        .await
        .map_err(|e| ModelError::Configuration(format!("AWS credentials unavailable: {e}")))?;
    let identity = credentials.into();
    let settings = SigningSettings::default();
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region.as_ref())
        .name("bedrock")
        .time(SystemTime::now())
        .settings(settings)
        .build()
        .map_err(|e| ModelError::Configuration(e.to_string()))?
        .into();
    let signable = SignableRequest::new(
        "POST",
        url,
        [
            ("content-type", "application/json"),
            ("accept", "application/vnd.amazon.eventstream"),
        ]
        .into_iter(),
        SignableBody::Bytes(&bytes),
    )
    .map_err(|e| ModelError::Configuration(e.to_string()))?;
    let (instructions, _) = sign(signable, &params)
        .map_err(|e| ModelError::Configuration(format!("AWS signing failed: {e}")))?
        .into_parts();
    let mut request = request;
    for (name, value) in instructions.headers() {
        request = request.header(name, value);
    }
    Ok(request.body(bytes))
}

fn parts(parts: &[ContentPart]) -> Result<Vec<Value>, ModelError> {
    parts
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => Ok(json!({"text":text})),
            ContentPart::Image { media_type, data } => {
                let format = media_type
                    .strip_prefix("image/")
                    .filter(|format| matches!(*format, "png" | "jpeg" | "gif" | "webp"))
                    .ok_or_else(|| {
                        ModelError::Configuration("unsupported Bedrock image format".into())
                    })?;
                Ok(json!({"image":{"format":format, "source":{"bytes":data}}}))
            }
        })
        .collect()
}

fn payload(provider: &NativeProvider, request: &ModelRequest) -> Result<Value, ModelError> {
    let mut system = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        if message.role == Role::System {
            system.push(json!({"text":message.content}));
            continue;
        }
        let role = if message.role == Role::Assistant {
            "assistant"
        } else {
            "user"
        };
        let mut content = Vec::new();
        if message.role == Role::Tool {
            let mut result = vec![json!({"text":message.content})];
            result.extend(parts(&message.parts)?);
            content
                .push(json!({"toolResult":{"toolUseId":message.tool_call_id, "content":result}}));
        } else if let Some(saved) = provider.replay(message).and_then(Value::as_array) {
            content.clone_from(saved);
        } else {
            if !message.content.is_empty() {
                content.push(json!({"text":message.content}));
            }
            content.extend(parts(&message.parts)?);
            for tool in &message.tool_calls {
                content.push(json!({"toolUse":{"toolUseId":tool.id, "name":tool.function.name, "input":arguments(tool)?}}));
            }
        }
        if content.is_empty() {
            continue;
        }
        if let Some(last) = messages.last_mut().filter(|m| m["role"] == role) {
            last["content"].as_array_mut().unwrap().extend(content);
        } else {
            messages.push(json!({"role":role, "content":content}));
        }
    }
    let max = provider.config.max_output_tokens.unwrap_or(8192);
    let mut body = json!({"messages":messages, "inferenceConfig":{"maxTokens":max}});
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["toolConfig"] = json!({"tools":request.tools.iter().map(|t| json!({"toolSpec":{"name":t.function.name, "description":t.function.description, "inputSchema":{"json":t.function.parameters}}})).collect::<Vec<_>>()});
    }
    if let Some(effort) = provider
        .config
        .reasoning_effort
        .filter(|_| provider.model_id().contains("claude"))
    {
        if super::adaptive_thinking(provider.model_id()) {
            body["additionalModelRequestFields"] = json!({"thinking":{"type":"adaptive"}, "output_config":{"effort":super::adaptive_effort(effort)}});
        } else if max > 1024 {
            body["additionalModelRequestFields"] = json!({"thinking":{"type":"enabled", "budget_tokens":match effort { crate::ReasoningEffort::Low => 1024, crate::ReasoningEffort::Medium => 4096, _ => 8192 }.min(max-1)}});
        }
    }
    Ok(body)
}

pub(super) async fn stream(
    provider: &NativeProvider,
    request: ModelRequest,
    delta: &mut (dyn FnMut(String) + Send),
    thinking: &mut (dyn FnMut(String) + Send),
) -> Result<ModelResponse, ModelError> {
    let region = if provider.config.api_key.is_some() || env("AWS_BEARER_TOKEN_BEDROCK").is_some() {
        provider
            .config
            .region
            .clone()
            .or_else(|| env("AWS_REGION"))
            .or_else(|| env("AWS_DEFAULT_REGION"))
            .ok_or_else(|| {
                ModelError::Configuration(
                    "Bedrock bearer auth requires AWS_REGION or AWS_DEFAULT_REGION".into(),
                )
            })?
    } else {
        config(provider)
            .await
            .region()
            .map(|r| r.as_ref().to_owned())
            .ok_or_else(|| ModelError::Configuration("Bedrock region is not configured".into()))?
    };
    let base = provider.config.base_url.clone().unwrap_or_else(|| {
        format!(
            "https://bedrock-runtime.{region}.{}",
            if region.starts_with("cn-") {
                "amazonaws.com.cn"
            } else {
                "amazonaws.com"
            }
        )
    });
    let mut url = reqwest::Url::parse(&format!("{}/", base.trim_end_matches('/')))
        .map_err(|e| ModelError::Configuration(e.to_string()))?;
    url.path_segments_mut()
        .map_err(|()| ModelError::Configuration("invalid Bedrock base URL".into()))?
        .pop_if_empty()
        .push("model")
        .push(provider.model_id())
        .push("converse-stream");
    let bytes = serde_json::to_vec(&payload(provider, &request)?)
        .map_err(|e| ModelError::InvalidResponse(e.to_string()))?;
    let response = transport::success(
        authenticated(provider, url.as_str(), bytes)
            .await?
            .send()
            .await?,
    )
    .await?;
    let mut stream = response.bytes_stream();
    let mut pending = Vec::new();
    let mut state = StreamState::default();
    while let Some(chunk) = stream.next().await {
        pending.extend_from_slice(&chunk?);
        while pending.len() >= 12 {
            let size = u32::from_be_bytes(pending[..4].try_into().unwrap()) as usize;
            if !(16..=8 * 1024 * 1024).contains(&size) {
                return Err(ModelError::InvalidResponse(
                    "invalid AWS event frame size".into(),
                ));
            }
            if pending.len() < size {
                break;
            }
            let frame = aws_smithy_eventstream::frame::read_message_from(&pending[..size])
                .map_err(|e| {
                    ModelError::InvalidResponse(format!("invalid AWS event frame: {e}"))
                })?;
            let header = |name: &str| {
                frame
                    .headers()
                    .iter()
                    .find(|h| h.name().as_str() == name)
                    .and_then(|h| h.value().as_string().ok())
                    .map(aws_smithy_types::str_bytes::StrBytes::as_str)
                    .unwrap_or_default()
            };
            let kind = header(":event-type");
            let event: Value = serde_json::from_slice(frame.payload())
                .map_err(|e| ModelError::InvalidResponse(e.to_string()))?;
            if header(":message-type") == "exception" || header(":message-type") == "error" {
                return Err(embedded_error(header(":exception-type"), &event));
            }
            state.apply(kind, &event, delta, thinking)?;
            pending.drain(..size);
        }
    }
    if !pending.is_empty() {
        return Err(ModelError::InvalidResponse(
            "Bedrock binary stream was truncated".into(),
        ));
    }
    state.finish(provider)
}

#[derive(Default)]
struct StreamState {
    blocks: BTreeMap<usize, Value>,
    inputs: BTreeMap<usize, String>,
    output: ModelResponse,
}
impl StreamState {
    fn apply(
        &mut self,
        kind: &str,
        event: &Value,
        delta: &mut (dyn FnMut(String) + Send),
        thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<(), ModelError> {
        let index = super::index(event, "contentBlockIndex")?;
        match kind {
            "contentBlockStart" => {
                if let Some(tool) = event["start"].get("toolUse") {
                    self.blocks.insert(index, json!({"toolUse":{"toolUseId":tool["toolUseId"], "name":tool["name"], "input":{}}}));
                }
            }
            "contentBlockDelta" => {
                let change = &event["delta"];
                if let Some(text) = change["text"].as_str() {
                    delta(text.into());
                    let block = self
                        .blocks
                        .entry(index)
                        .or_insert_with(|| json!({"text":""}));
                    block["text"] = json!(format!(
                        "{}{}",
                        block["text"].as_str().unwrap_or_default(),
                        text
                    ));
                } else if let Some(input) = change["toolUse"]["input"].as_str() {
                    self.inputs.entry(index).or_default().push_str(input);
                } else if let Some(reasoning) = change.get("reasoningContent") {
                    let block = self.blocks.entry(index).or_insert_with(|| json!({"reasoningContent":{"reasoningText":{"text":"", "signature":""}}}));
                    for field in ["text", "signature"] {
                        if let Some(text) = reasoning[field].as_str() {
                            if field == "text" {
                                thinking(text.into());
                            }
                            block["reasoningContent"]["reasoningText"][field] = json!(format!(
                                "{}{}",
                                block["reasoningContent"]["reasoningText"][field]
                                    .as_str()
                                    .unwrap_or_default(),
                                text
                            ));
                        }
                    }
                    if !reasoning["redactedContent"].is_null() {
                        block["reasoningContent"] =
                            json!({"redactedContent":reasoning["redactedContent"]});
                    }
                }
            }
            "contentBlockStop" => {
                if let Some(input) = self.inputs.remove(&index) {
                    let block = self.blocks.get_mut(&index).ok_or_else(|| {
                        ModelError::InvalidResponse("Bedrock tool delta has no start".into())
                    })?;
                    block["toolUse"]["input"] = serde_json::from_str(&input)
                        .map_err(|e| ModelError::InvalidResponse(e.to_string()))?;
                }
            }
            "messageStop" => {
                self.output.finish_reason = event["stopReason"].as_str().map(str::to_owned);
            }
            "metadata" => {
                self.output.usage = Some(event["usage"].clone());
            }
            _ => {}
        }
        Ok(())
    }
    fn finish(mut self, provider: &NativeProvider) -> Result<ModelResponse, ModelError> {
        if !self.inputs.is_empty() || self.output.finish_reason.is_none() {
            return Err(ModelError::InvalidResponse(
                "Bedrock stream was truncated".into(),
            ));
        }
        let blocks: Vec<_> = self.blocks.into_values().collect();
        for block in &blocks {
            if let Some(text) = block["text"].as_str() {
                self.output.content.push_str(text);
            }
            if let Some(tool) = block.get("toolUse") {
                self.output.tool_calls.push(call(
                    string(tool, "toolUseId"),
                    string(tool, "name"),
                    &tool["input"],
                ));
            }
        }
        self.output.provider_metadata = Some(provider.metadata(json!(blocks)));
        Ok(self.output)
    }
}

fn embedded_error(name: &str, event: &Value) -> ModelError {
    let status = match name {
        "throttlingException" => 429,
        "serviceUnavailableException" => 503,
        "internalServerException" => 500,
        _ => event["originalStatusCode"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(400),
    };
    ModelError::HttpResponse {
        status,
        message: event.to_string(),
        retry_after: None,
    }
}
