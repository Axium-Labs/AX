//! Shared adapter construction used by inference and catalog discovery.
use crate::{
    ModelError, ModelProvider, NativeConfig, NativeProvider, OpenAiCompatibleConfig,
    OpenAiCompatibleProvider, OpenAiConfig, OpenAiProvider, ProviderProtocol, provider,
};
use std::sync::Arc;

/// Creates an adapter without network I/O or credential discovery.
/// # Errors
/// Returns an actionable error for missing endpoint/key configuration.
pub fn provider_adapter(config: NativeConfig) -> Result<Arc<dyn ModelProvider>, ModelError> {
    provider_adapter_with_client(config, reqwest::Client::new())
}

/// Construct an adapter using an explicit HTTP transport.
/// # Errors
/// Returns missing or invalid provider configuration.
pub fn provider_adapter_with_client(
    config: NativeConfig,
    client: reqwest::Client,
) -> Result<Arc<dyn ModelProvider>, ModelError> {
    let spec = provider(&config.provider_id).ok_or_else(|| {
        ModelError::Configuration(format!("unknown provider: {}", config.provider_id))
    })?;
    if matches!(
        spec.protocol,
        ProviderProtocol::Anthropic
            | ProviderProtocol::Google
            | ProviderProtocol::Bedrock
            | ProviderProtocol::PiMessages
    ) {
        return Ok(Arc::new(NativeProvider::with_client(config, client)?));
    }
    let key = config.api_key.clone().ok_or_else(|| {
        ModelError::Configuration(format!("{} API key is not configured", spec.name))
    })?;
    if spec.id == "azure-openai-responses" {
        let endpoint = match config.base_url.clone() {
            Some(base) => azure_endpoint(&base)?,
            None => azure_endpoint_from_env()?,
        };
        let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT_NAME_MAP")
            .ok()
            .and_then(|mapping| {
                mapping.split(',').find_map(|entry| {
                    let (model, deployment) = entry.split_once('=')?;
                    (model.trim() == config.model).then(|| deployment.trim().to_owned())
                })
            });
        let mut adapter = OpenAiConfig::from_azure(config.model, key, endpoint, deployment)?;
        adapter.context_window = config.context_window;
        adapter.max_output_tokens = config.max_output_tokens;
        adapter.reasoning_effort = config.reasoning_effort;
        return Ok(Arc::new(OpenAiProvider::with_client(adapter, client)));
    }
    if spec.protocol != ProviderProtocol::OpenAiCompatible {
        return Err(ModelError::Configuration(format!(
            "{} has no enabled adapter",
            spec.name
        )));
    }
    let endpoint = config
        .base_url
        .clone()
        .or_else(|| crate::provider_chat_endpoint(spec.id))
        .ok_or_else(|| {
            ModelError::Configuration(
                crate::provider_configuration_reason(spec.id)
                    .unwrap_or("provider endpoint is not configured")
                    .into(),
            )
        })?;
    let mut adapter =
        OpenAiCompatibleConfig::new(spec.id, config.model, key, endpoint, config.context_window);
    adapter.max_output_tokens = config.max_output_tokens;
    adapter.reasoning_effort = config.reasoning_effort;
    if spec.id == "cloudflare-ai-gateway" {
        adapter.api_key_header = "cf-aig-authorization".into();
    }
    Ok(Arc::new(OpenAiCompatibleProvider::with_client(
        adapter, client,
    )))
}

/// Normalize an Azure resource root or API base to a Responses endpoint.
/// # Errors
/// Returns an error for an invalid HTTP URL.
pub fn azure_endpoint(base: &str) -> Result<String, ModelError> {
    let mut url = reqwest::Url::parse(base.trim())
        .map_err(|e| ModelError::Configuration(format!("invalid Azure base URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ModelError::Configuration(
            "Azure base URL must be HTTP".into(),
        ));
    }
    let path = url.path().trim_end_matches('/');
    let normalized = match path {
        "" | "/" | "/openai" => "/openai/v1/responses".to_owned(),
        path if path.ends_with("/responses") => path.to_owned(),
        path => format!("{path}/responses"),
    };
    url.set_path(&normalized);
    if let Ok(version) = std::env::var("AZURE_OPENAI_API_VERSION")
        && !version.is_empty()
        && version != "v1"
    {
        url.query_pairs_mut().append_pair("api-version", &version);
    }
    Ok(url.into())
}

fn azure_endpoint_from_env() -> Result<String, ModelError> {
    if let Ok(base) = std::env::var("AZURE_OPENAI_BASE_URL")
        && !base.trim().is_empty()
    {
        return azure_endpoint(&base);
    }
    if let Ok(resource) = std::env::var("AZURE_OPENAI_RESOURCE_NAME")
        && !resource.is_empty()
    {
        if !resource
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(ModelError::Configuration(
                "invalid AZURE_OPENAI_RESOURCE_NAME".into(),
            ));
        }
        return azure_endpoint(&format!("https://{resource}.openai.azure.com"));
    }
    Err(ModelError::Configuration(
        "Azure requires AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME".into(),
    ))
}

/// Local ambient-auth hints only; AWS and Google credentials are refreshed
/// lazily by the adapters. A hint is not proof of online authorization.
#[must_use]
pub fn ambient_credentials_configured(id: &str) -> bool {
    match id {
        "amazon-bedrock" => crate::native::bedrock_configured(),
        "google-vertex" => crate::native::google_configured(),
        _ => false,
    }
}
