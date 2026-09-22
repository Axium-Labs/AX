use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    FunctionCall, Message, ModelError, ModelInfo, ModelProvider, ModelRequest, ModelResponse,
    ReasoningEffort, Role, ToolCall, ToolSpec,
};

const API_ENDPOINT: &str = "https://api.openai.com/v1/responses";
const CODEX_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const FALLBACK_MODEL: &str = "gpt-5.6-sol";

#[derive(Clone, Debug)]
enum CredentialSource {
    ApiKey(String),
    CodexFile(PathBuf),
    OAuth {
        access: String,
        account_id: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct OpenAiConfig {
    pub model: String,
    pub endpoint: String,
    pub context_window: usize,
    pub reasoning_effort: Option<ReasoningEffort>,
    credentials: CredentialSource,
    codex_catalog_path: Option<PathBuf>,
    is_codex: bool,
}

impl OpenAiConfig {
    #[must_use]
    pub fn from_api_key(model: Option<String>, api_key: String) -> Self {
        Self {
            model: model.unwrap_or_else(|| FALLBACK_MODEL.to_owned()),
            endpoint: std::env::var("OPENAI_API_URL").unwrap_or_else(|_| API_ENDPOINT.to_owned()),
            context_window: 200_000,
            reasoning_effort: None,
            credentials: CredentialSource::ApiKey(api_key),
            codex_catalog_path: None,
            is_codex: false,
        }
    }

    /// Creates standard `OpenAI` API-key configuration from `OPENAI_API_KEY`.
    ///
    /// # Errors
    ///
    /// Returns an error when the environment variable is absent.
    pub fn from_env(model: Option<String>) -> Result<Self, ModelError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| ModelError::Configuration("OPENAI_API_KEY is not set".to_owned()))?;
        Ok(Self {
            model: model.unwrap_or_else(|| FALLBACK_MODEL.to_owned()),
            endpoint: std::env::var("OPENAI_API_URL").unwrap_or_else(|_| API_ENDPOINT.to_owned()),
            context_window: 200_000,
            reasoning_effort: None,
            credentials: CredentialSource::ApiKey(api_key),
            codex_catalog_path: None,
            is_codex: false,
        })
    }

    /// Creates a ChatGPT-backed provider by reading an explicitly supplied
    /// legacy Codex auth cache.
    /// The token file is re-read for every request so refreshes made by Codex
    /// are observed by a long-running `ax` process.
    ///
    /// # Errors
    ///
    /// Returns an error when no home directory or auth file can be resolved.
    pub fn from_codex_auth(
        model: Option<String>,
        auth_path: Option<PathBuf>,
    ) -> Result<Self, ModelError> {
        let auth_path = auth_path.ok_or_else(|| {
            ModelError::Configuration(
                "legacy Codex auth requires an explicit --codex-auth path".to_owned(),
            )
        })?;
        if !auth_path.is_file() {
            return Err(ModelError::Configuration(format!(
                "Codex auth cache not found at {}; run `codex login` or configure file-based credential storage",
                auth_path.display()
            )));
        }
        let initial_credentials = load_codex_credentials(&auth_path)?;
        let codex_catalog_path = auth_path
            .parent()
            .map(|parent| parent.join("models_cache.json"));
        let default_endpoint = if initial_credentials.api_key {
            API_ENDPOINT
        } else {
            CODEX_ENDPOINT
        };
        Ok(Self {
            model: model.unwrap_or_else(|| FALLBACK_MODEL.to_owned()),
            endpoint: std::env::var("CODEX_API_URL")
                .unwrap_or_else(|_| default_endpoint.to_owned()),
            context_window: 200_000,
            reasoning_effort: None,
            credentials: CredentialSource::CodexFile(auth_path),
            codex_catalog_path,
            is_codex: true,
        })
    }

    /// Creates a ChatGPT-backed provider from an OAuth credential owned by AX.
    #[must_use]
    pub fn from_oauth(model: Option<String>, access: String, account_id: Option<String>) -> Self {
        let account_id = account_id.or_else(|| crate::account_id_from_token(&access));
        Self {
            model: model.unwrap_or_else(|| FALLBACK_MODEL.to_owned()),
            endpoint: std::env::var("CODEX_API_URL").unwrap_or_else(|_| CODEX_ENDPOINT.to_owned()),
            context_window: 200_000,
            reasoning_effort: None,
            credentials: CredentialSource::OAuth { access, account_id },
            codex_catalog_path: None,
            is_codex: true,
        }
    }
}

pub struct OpenAiProvider {
    client: reqwest::Client,
    config: OpenAiConfig,
}

impl OpenAiProvider {
    #[must_use]
    pub fn new(config: OpenAiConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    fn auth_headers(&self) -> Result<HeaderMap, ModelError> {
        let credentials = match &self.config.credentials {
            CredentialSource::ApiKey(key) => ResolvedCredentials {
                token: key.clone(),
                account_id: None,
                api_key: true,
            },
            CredentialSource::CodexFile(path) => load_codex_credentials(path)?,
            CredentialSource::OAuth { access, account_id } => ResolvedCredentials {
                token: access.clone(),
                account_id: account_id.clone(),
                api_key: false,
            },
        };
        let mut headers = HeaderMap::new();
        let authorization = HeaderValue::from_str(&format!("Bearer {}", credentials.token))
            .map_err(|error| ModelError::Configuration(error.to_string()))?;
        headers.insert(AUTHORIZATION, authorization);
        if let Some(account_id) = credentials.account_id {
            headers.insert(
                "chatgpt-account-id",
                HeaderValue::from_str(&account_id)
                    .map_err(|error| ModelError::Configuration(error.to_string()))?,
            );
        }
        Ok(headers)
    }
}

#[derive(Deserialize)]
struct CodexAuthFile {
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<String>,
    tokens: Option<CodexTokens>,
}

#[derive(Deserialize)]
struct CodexTokens {
    access_token: String,
    account_id: Option<String>,
}

struct ResolvedCredentials {
    token: String,
    account_id: Option<String>,
    api_key: bool,
}

fn load_codex_credentials(path: &PathBuf) -> Result<ResolvedCredentials, ModelError> {
    let contents = fs::read_to_string(path).map_err(|error| {
        ModelError::Configuration(format!("failed to read {}: {error}", path.display()))
    })?;
    let auth = serde_json::from_str::<CodexAuthFile>(&contents).map_err(|error| {
        ModelError::Configuration(format!(
            "invalid Codex auth cache {}: {error}",
            path.display()
        ))
    })?;
    if let Some(api_key) = auth.api_key.filter(|key| !key.trim().is_empty()) {
        return Ok(ResolvedCredentials {
            token: api_key,
            account_id: None,
            api_key: true,
        });
    }
    let tokens = auth.tokens.ok_or_else(|| {
        ModelError::Configuration("Codex auth cache contains no usable credentials".to_owned())
    })?;
    Ok(ResolvedCredentials {
        token: tokens.access_token,
        account_id: tokens.account_id,
        api_key: false,
    })
}

#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    input: Vec<Value>,
    tools: Vec<Value>,
    store: bool,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<Value>,
}

#[derive(Deserialize)]
struct ResponsesResponse {
    #[serde(default)]
    output: Vec<Value>,
    status: Option<String>,
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn name(&self) -> &str {
        if self.config.is_codex {
            "openai-codex"
        } else {
            "openai"
        }
    }

    fn model_id(&self) -> &str {
        &self.config.model
    }

    fn context_window(&self) -> usize {
        self.config.context_window
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let payload = ResponsesRequest {
            model: &self.config.model,
            input: response_input(&request.messages),
            tools: response_tools(&request.tools),
            store: false,
            stream: false,
            reasoning: self
                .config
                .reasoning_effort
                .map(|effort| json!({ "effort": effort })),
        };
        let response = self
            .client
            .post(&self.config.endpoint)
            .headers(self.auth_headers()?)
            .header("originator", "ax")
            .json(&payload)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "response body unavailable".to_owned());
            return Err(ModelError::HttpStatus {
                status: status.as_u16(),
                message: truncate_error(&message),
            });
        }
        let response = response.json::<ResponsesResponse>().await?;
        parse_response(response)
    }

    async fn complete_stream(
        &self,
        request: ModelRequest,
        on_delta: &mut (dyn FnMut(String) + Send),
        on_thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        let _ = on_thinking;
        let payload = ResponsesRequest {
            model: &self.config.model,
            input: response_input(&request.messages),
            tools: response_tools(&request.tools),
            store: false,
            stream: true,
            reasoning: self
                .config
                .reasoning_effort
                .map(|effort| json!({ "effort": effort })),
        };
        let response = self
            .client
            .post(&self.config.endpoint)
            .headers(self.auth_headers()?)
            .header("originator", "ax")
            .json(&payload)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "response body unavailable".to_owned());
            return Err(ModelError::HttpStatus {
                status: status.as_u16(),
                message: truncate_error(&message),
            });
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut output = StreamedResponse::default();
        while let Some(chunk) = stream.next().await {
            buffer.extend_from_slice(&chunk?);
            while let Some((end, delimiter_len)) = find_sse_event_end(&buffer) {
                let event = buffer.drain(..end).collect::<Vec<_>>();
                buffer.drain(..delimiter_len);
                process_response_event(&event, &mut output, on_delta)?;
            }
        }
        if !buffer.is_empty() {
            process_response_event(&buffer, &mut output, on_delta)?;
        }
        output.finish()
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        if self.config.model == "catalog-only" {
            return self.load_codex_catalog();
        }
        match self.fetch_models().await {
            Ok(models) if !models.is_empty() => Ok(models),
            Ok(_) => self.load_codex_catalog(),
            Err(remote_error) => self.load_codex_catalog().map_err(|_| remote_error),
        }
    }

    fn fallback_models(&self) -> Vec<ModelInfo> {
        // Catalogs are access-specific. Do not advertise guessed model ids:
        // callers fall back to the last successful dynamic cache instead.
        let ids: &[&str] = &[];
        ids.iter()
            .map(|id| ModelInfo {
                id: (*id).to_owned(),
                display_name: (*id).to_owned(),
                provider: if self.config.is_codex {
                    "openai-codex"
                } else {
                    "openai"
                }
                .to_owned(),
                context_window: 200_000,
                reasoning_efforts: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                default_reasoning_effort: Some(ReasoningEffort::Medium),
                supports_tools: true,
                endpoint: None,
            })
            .collect()
    }
}

impl OpenAiProvider {
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let endpoint = self
            .config
            .endpoint
            .trim_end_matches("/responses")
            .trim_end_matches('/')
            .to_owned()
            + "/models";
        let mut request = self
            .client
            .get(endpoint)
            .timeout(Duration::from_secs(10))
            .headers(self.auth_headers()?);
        if self.config.is_codex {
            // ChatGPT's Codex catalog endpoint requires the same compatibility
            // parameters as the Codex CLI. Without client_version the request
            // can authenticate successfully while returning no usable catalog.
            let client_version =
                std::env::var("AX_CODEX_CLIENT_VERSION").unwrap_or_else(|_| "0.154.0".to_owned());
            request = request
                .query(&[("client_version", client_version.as_str())])
                .header("originator", "codex_cli_rs")
                .header("user-agent", format!("codex-cli/{client_version}"));
        } else {
            request = request.header("originator", "ax");
        }
        let response = request
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        parse_models_catalog(&response, self.name())
    }

    fn load_codex_catalog(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let path = self.config.codex_catalog_path.as_ref().ok_or_else(|| {
            ModelError::InvalidResponse("no native Codex model catalog is configured".to_owned())
        })?;
        let value = serde_json::from_slice::<Value>(&fs::read(path)?).map_err(|error| {
            ModelError::InvalidResponse(format!("invalid Codex model catalog: {error}"))
        })?;
        parse_models_catalog(&value, "openai-codex")
    }
}

fn parse_models_catalog(value: &Value, provider: &str) -> Result<Vec<ModelInfo>, ModelError> {
    let models = value
        .get("models")
        .or_else(|| value.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| ModelError::InvalidResponse("model catalog contains no list".to_owned()))?;
    Ok(models
        .iter()
        .filter(|model| model.get("visibility").and_then(Value::as_str) != Some("hide"))
        .filter_map(|model| {
            let id = model
                .get("slug")
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)?
                .to_owned();
            let efforts = model
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|level| {
                    level
                        .get("effort")
                        .and_then(Value::as_str)
                        .and_then(ReasoningEffort::parse)
                })
                .collect::<Vec<_>>();
            Some(ModelInfo {
                display_name: model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_owned(),
                id,
                provider: provider.to_owned(),
                context_window: model
                    .get("context_window")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(200_000),
                reasoning_efforts: efforts,
                default_reasoning_effort: model
                    .get("default_reasoning_level")
                    .and_then(Value::as_str)
                    .and_then(ReasoningEffort::parse),
                supports_tools: model.get("supported_in_api").and_then(Value::as_bool)
                    != Some(false),
                endpoint: None,
            })
        })
        .collect())
}

#[derive(Default)]
struct StreamedResponse {
    content: String,
    calls: BTreeMap<String, StreamedCall>,
    status: Option<String>,
}

#[derive(Default)]
struct StreamedCall {
    call_id: String,
    name: String,
    arguments: String,
}

impl StreamedResponse {
    fn finish(self) -> Result<ModelResponse, ModelError> {
        if self.content.is_empty() && self.calls.is_empty() {
            return Err(ModelError::InvalidResponse(
                "Responses API stream contains no text or function calls".to_owned(),
            ));
        }
        Ok(ModelResponse {
            content: self.content,
            tool_calls: self
                .calls
                .into_values()
                .map(|call| ToolCall {
                    id: call.call_id,
                    kind: "function".to_owned(),
                    function: FunctionCall {
                        name: call.name,
                        arguments: call.arguments,
                    },
                })
                .collect(),
            finish_reason: self.status,
        })
    }
}

fn find_sse_event_end(buffer: &[u8]) -> Option<(usize, usize)> {
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

fn process_response_event(
    event: &[u8],
    output: &mut StreamedResponse,
    on_delta: &mut (dyn FnMut(String) + Send),
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
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }
    let value = serde_json::from_str::<Value>(data)
        .map_err(|error| ModelError::InvalidResponse(error.to_string()))?;
    match value.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                output.content.push_str(delta);
                on_delta(delta.to_owned());
            }
        }
        Some("response.output_item.added" | "response.output_item.done") => {
            if let Some(item) = value.get("item")
                && item.get("type").and_then(Value::as_str) == Some("function_call")
            {
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let call = output.calls.entry(item_id).or_default();
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    call_id.clone_into(&mut call.call_id);
                }
                if let Some(name) = item.get("name").and_then(Value::as_str) {
                    name.clone_into(&mut call.name);
                }
                if let Some(arguments) = item.get("arguments").and_then(Value::as_str)
                    && !arguments.is_empty()
                {
                    arguments.clone_into(&mut call.arguments);
                }
            }
        }
        Some("response.function_call_arguments.delta") => {
            let item_id = value
                .get("item_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                output
                    .calls
                    .entry(item_id)
                    .or_default()
                    .arguments
                    .push_str(delta);
            }
        }
        Some("response.completed") => {
            output.status = value
                .get("response")
                .and_then(|response| response.get("status"))
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        Some("error") => {
            return Err(ModelError::InvalidResponse(
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown streaming error")
                    .to_owned(),
            ));
        }
        _ => {}
    }
    Ok(())
}

fn response_input(messages: &[Message]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        match message.role {
            Role::Tool => input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id,
                "output": message.content
            })),
            Role::Assistant if !message.tool_calls.is_empty() => {
                if !message.content.is_empty() {
                    input.push(json!({ "role": "assistant", "content": message.content }));
                }
                input.extend(message.tool_calls.iter().map(|call| {
                    json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.function.name,
                        "arguments": call.function.arguments
                    })
                }));
            }
            Role::System => {
                input.push(json!({ "role": "developer", "content": message.content }));
            }
            Role::User | Role::Assistant => {
                input.push(json!({ "role": message.role, "content": message.content }));
            }
        }
    }
    input
}

fn response_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.function.name,
                "description": tool.function.description,
                "parameters": tool.function.parameters,
                "strict": false
            })
        })
        .collect()
}

fn parse_response(response: ResponsesResponse) -> Result<ModelResponse, ModelError> {
    let mut content = Vec::new();
    let mut tool_calls = Vec::new();
    for item in response.output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    content.extend(parts.iter().filter_map(|part| {
                        (part.get("type").and_then(Value::as_str) == Some("output_text"))
                            .then(|| part.get("text").and_then(Value::as_str))
                            .flatten()
                            .map(str::to_owned)
                    }));
                }
            }
            Some("function_call") => tool_calls.push(ToolCall {
                id: item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                kind: "function".to_owned(),
                function: FunctionCall {
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    arguments: item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_owned(),
                },
            }),
            _ => {}
        }
    }
    if content.is_empty() && tool_calls.is_empty() {
        return Err(ModelError::InvalidResponse(
            "Responses API output contains no text or function calls".to_owned(),
        ));
    }
    Ok(ModelResponse {
        content: content.join("\n"),
        tool_calls,
        finish_reason: response.status,
    })
}

fn truncate_error(message: &str) -> String {
    message.chars().take(1_000).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_and_function_calls() {
        let response = ResponsesResponse {
            output: vec![
                json!({
                    "type": "message",
                    "content": [{"type":"output_text","text":"hello"}]
                }),
                json!({
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "shell",
                    "arguments": "{\"command\":\"pwd\"}"
                }),
            ],
            status: Some("completed".to_owned()),
        };
        let parsed = parse_response(response).expect("response should parse");
        assert_eq!(parsed.content, "hello");
        assert_eq!(parsed.tool_calls[0].function.name, "shell");
    }

    #[test]
    fn combines_responses_api_stream_events() {
        let mut streamed = StreamedResponse::default();
        let mut deltas = Vec::new();
        let mut on_delta = |delta: String| deltas.push(delta);
        for event in [
            br#"data: {"type":"response.output_item.added","item":{"id":"item-1","type":"function_call","call_id":"call-1","name":"shell","arguments":""}}"#.as_slice(),
            br#"data: {"type":"response.function_call_arguments.delta","item_id":"item-1","delta":"{\"command\":\"pwd\"}"}"#.as_slice(),
            br#"data: {"type":"response.output_text.delta","delta":"hello"}"#.as_slice(),
            br#"data: {"type":"response.completed","response":{"status":"completed"}}"#.as_slice(),
        ] {
            process_response_event(event, &mut streamed, &mut on_delta)
                .expect("stream event should parse");
        }
        let response = streamed.finish().expect("stream should finish");
        assert_eq!(deltas, ["hello"]);
        assert_eq!(response.content, "hello");
        assert_eq!(response.tool_calls[0].function.name, "shell");
        assert_eq!(
            response.tool_calls[0].function.arguments,
            r#"{"command":"pwd"}"#
        );
        assert_eq!(response.finish_reason.as_deref(), Some("completed"));
        assert_eq!(find_sse_event_end(b"one\r\n\r\ntwo"), Some((3, 4)));
    }

    #[test]
    fn loads_codex_file_credentials_without_exposing_them() {
        let path = std::env::temp_dir().join(format!(
            "ax-codex-auth-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should follow Unix epoch")
                .as_nanos()
        ));
        fs::write(
            &path,
            r#"{"tokens":{"access_token":"secret-token","account_id":"account-1"}}"#,
        )
        .expect("temporary auth file should be written");
        let credentials = load_codex_credentials(&path).expect("credentials should load");
        fs::remove_file(path).expect("temporary auth file should be removed");
        assert_eq!(credentials.token, "secret-token");
        assert_eq!(credentials.account_id.as_deref(), Some("account-1"));
        assert!(!credentials.api_key);
    }
}
