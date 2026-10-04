//! Native wire adapters. Constructors only inspect local configuration; auth
//! discovery and refresh run on the first request, never during AX startup.
mod anthropic;
mod bedrock;
mod google;
mod radius;
mod transport;

use crate::{
    ModelCapabilities, ModelError, ModelInfo, ModelProvider, ModelRequest, ModelResponse,
    ReasoningEffort, builtin_models, stats,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

#[derive(Clone, Debug)]
pub struct NativeConfig {
    pub provider_id: String,
    pub model: String,
    pub api_key: Option<String>,
    /// Optional full base URL, useful for gateways and explicit test servers.
    pub base_url: Option<String>,
    pub context_window: usize,
    pub region: Option<String>,
    pub aws_credentials: Option<aws_credential_types::Credentials>,
    pub google_credentials_file: Option<std::path::PathBuf>,
    pub max_output_tokens: Option<usize>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl NativeConfig {
    #[must_use]
    pub fn new(
        provider_id: impl Into<String>,
        model: String,
        api_key: Option<String>,
        context_window: usize,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model,
            api_key,
            context_window,
            base_url: None,
            max_output_tokens: None,
            reasoning_effort: None,
            region: None,
            aws_credentials: None,
            google_credentials_file: None,
        }
    }
}

pub struct NativeProvider {
    config: NativeConfig,
    client: reqwest::Client,
    google_auth: google::GoogleAuth,
    aws: tokio::sync::OnceCell<aws_config::SdkConfig>,
    radius_base: tokio::sync::OnceCell<String>,
}

impl NativeProvider {
    /// # Errors
    /// Returns a configuration error for unknown providers or invalid URLs.
    pub fn new(config: NativeConfig) -> Result<Self, ModelError> {
        Self::with_client(config, reqwest::Client::new())
    }

    /// Use an explicitly configured HTTP transport (including proxy policy).
    /// # Errors
    /// Returns the same configuration errors as `new`.
    pub fn with_client(config: NativeConfig, client: reqwest::Client) -> Result<Self, ModelError> {
        if !matches!(
            config.provider_id.as_str(),
            "anthropic" | "google" | "google-vertex" | "amazon-bedrock" | "radius"
        ) {
            return Err(ModelError::Configuration(format!(
                "unknown native provider: {}",
                config.provider_id
            )));
        }
        if let Some(base) = &config.base_url {
            let url =
                reqwest::Url::parse(base).map_err(|e| ModelError::Configuration(e.to_string()))?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(ModelError::Configuration(
                    "provider endpoint must be an HTTP URL".into(),
                ));
            }
        }
        Ok(Self {
            config,
            client,
            google_auth: google::GoogleAuth::default(),
            aws: tokio::sync::OnceCell::new(),
            radius_base: tokio::sync::OnceCell::new(),
        })
    }

    async fn stream(
        &self,
        request: ModelRequest,
        delta: &mut (dyn FnMut(String) + Send),
        thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        match self.name() {
            "anthropic" => anthropic::stream(self, request, delta, thinking).await,
            "google" | "google-vertex" => google::stream(self, request, delta, thinking).await,
            "amazon-bedrock" => bedrock::stream(self, request, delta, thinking).await,
            "radius" => radius::stream(self, request, delta, thinking).await,
            _ => unreachable!(),
        }
    }

    fn metadata(&self, content: Value) -> Value {
        {
            let mut metadata = json!({"provider": self.name(), "model": self.model_id()});
            metadata["content"] = content;
            metadata
        }
    }

    fn replay<'a>(&self, message: &'a crate::Message) -> Option<&'a Value> {
        let data = message.provider_metadata.as_ref()?;
        (data["provider"] == self.name() && data["model"] == self.model_id())
            .then_some(&data["content"])
    }
}

#[async_trait]
impl ModelProvider for NativeProvider {
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
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            tool_calling: true,
            vision: matches!(self.name(), "anthropic" | "google" | "google-vertex")
                || (self.name() == "amazon-bedrock" && self.model_id().contains("claude")),
        }
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.complete_stream(request, &mut |_| {}, &mut |_| {})
            .await
    }
    async fn complete_stream(
        &self,
        request: ModelRequest,
        delta: &mut (dyn FnMut(String) + Send),
        thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        let started = Instant::now();
        let first = AtomicBool::new(false);
        let mut observe = |text: String| {
            if !first.swap(true, Ordering::Relaxed) {
                stats::record_ttft(self.name(), self.model_id(), started.elapsed());
            }
            delta(text);
        };
        let mut observe_thinking = |text: String| {
            if !first.swap(true, Ordering::Relaxed) {
                stats::record_ttft(self.name(), self.model_id(), started.elapsed());
            }
            thinking(text);
        };
        let result = self
            .stream(request, &mut observe, &mut observe_thinking)
            .await;
        match &result {
            Ok(response) => stats::record_completion(
                self.name(),
                self.model_id(),
                (response.content.len() / 4) as u64,
                started.elapsed(),
            ),
            Err(_) => stats::record_error(self.name(), self.model_id()),
        }
        result
    }
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
        match self.name() {
            "anthropic" => anthropic::models(self).await,
            "google" => google::models(self).await,
            "radius" => radius::models(self).await,
            // These catalogs require a separate management-plane permission.
            // Preserve fallback/cache provenance rather than calling it live.
            _ => Err(ModelError::Configuration("this provider uses a local model catalog; model access is verified only by inference".into())),
        }
    }
    fn fallback_models(&self) -> Vec<ModelInfo> {
        builtin_models(self.name())
    }
}

pub(super) fn string(value: &Value, name: &str) -> String {
    value[name].as_str().unwrap_or_default().to_owned()
}
pub(super) fn call(id: String, name: String, args: &Value) -> crate::ToolCall {
    crate::ToolCall {
        id,
        kind: "function".into(),
        function: crate::FunctionCall {
            name,
            arguments: args.to_string(),
        },
    }
}
pub(super) fn arguments(call: &crate::ToolCall) -> Result<Value, ModelError> {
    serde_json::from_str(&call.function.arguments)
        .map_err(|e| ModelError::InvalidResponse(format!("invalid tool arguments: {e}")))
}

pub(crate) use bedrock::configured as bedrock_configured;
pub(crate) use google::configured as google_configured;

pub(super) fn index(event: &Value, field: &str) -> Result<usize, ModelError> {
    usize::try_from(event[field].as_u64().unwrap_or_default())
        .map_err(|_| ModelError::InvalidResponse("content block index is too large".into()))
}

/// Claude 4.6 and later use the adaptive API; older models use token budgets.
pub(super) fn adaptive_thinking(model: &str) -> bool {
    let Some(claude) = model.split("claude-").nth(1) else {
        return false;
    };
    let mut parts = claude.split('-');
    while let Some(part) = parts.next() {
        if let Ok(major) = part.parse::<u32>() {
            let minor = parts
                .next()
                .and_then(|part| part.parse::<u32>().ok())
                .unwrap_or(0);
            return major >= 5 || (major == 4 && minor >= 6);
        }
    }
    false
}
pub(super) fn adaptive_effort(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::Max => "max",
        _ => "high",
    }
}
