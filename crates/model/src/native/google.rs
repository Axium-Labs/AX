use super::{NativeProvider, arguments, call, string, transport};
use crate::ModelProvider;
use crate::{
    ContentPart, ModelError, ModelInfo, ModelRequest, ModelResponse, Role,
    providers::compatible_model_info,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
pub(super) struct GoogleAuth {
    token: tokio::sync::Mutex<Option<GoogleToken>>,
    #[cfg(test)]
    token_endpoint: Option<String>,
}

#[derive(Clone)]
struct GoogleToken {
    access: String,
    expires: Instant,
    project: Option<String>,
    quota: Option<String>,
}

pub(crate) fn adc_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        return Some(path.into());
    }
    #[cfg(windows)]
    let root = std::env::var("APPDATA").ok().map(PathBuf::from);
    #[cfg(not(windows))]
    let root = std::env::var("HOME")
        .ok()
        .map(|p| PathBuf::from(p).join(".config"));
    root.map(|root| root.join("gcloud/application_default_credentials.json"))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub(crate) fn configured() -> bool {
    env("GOOGLE_CLOUD_API_KEY").is_some()
        || adc_path().is_some_and(|path| path.is_file())
        || env("GOOGLE_CLOUD_PROJECT").is_some()
        || env("GCLOUD_PROJECT").is_some()
}

async fn access_token(provider: &NativeProvider) -> Result<GoogleToken, ModelError> {
    let mut token = provider.google_auth.token.lock().await;
    if let Some(cached) = token
        .as_ref()
        .filter(|cached| cached.expires > Instant::now() + Duration::from_secs(60))
    {
        return Ok(cached.clone());
    }
    let path = provider
        .config
        .google_credentials_file
        .clone()
        .or_else(adc_path);
    if (provider.config.google_credentials_file.is_some()
        || env("GOOGLE_APPLICATION_CREDENTIALS").is_some())
        && !path.as_ref().is_some_and(|path| path.is_file())
    {
        return Err(ModelError::Configuration(
            "Google ADC credential file does not exist".into(),
        ));
    }
    let credentials = match path.filter(|path| path.is_file()) {
        Some(path) => Some(
            serde_json::from_slice::<Value>(&tokio::fs::read(path).await?)
                .map_err(|e| ModelError::Configuration(format!("invalid Google ADC JSON: {e}")))?,
        ),
        None => None,
    };
    #[cfg(not(test))]
    let token_endpoint = "https://oauth2.googleapis.com/token";
    #[cfg(test)]
    let token_endpoint = provider
        .google_auth
        .token_endpoint
        .as_deref()
        .unwrap_or("https://oauth2.googleapis.com/token");
    let response = if let Some(credentials) = &credentials {
        exchange_credentials(provider, credentials, token_endpoint).await?
    } else {
        provider.client.get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token").header("Metadata-Flavor", "Google").timeout(Duration::from_secs(3)).send().await?
    };
    let response: Value = transport::success(response).await?.json().await?;
    let access = response["access_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ModelError::InvalidResponse("Google auth returned no access token".into()))?
        .to_owned();
    let project = credentials
        .as_ref()
        .and_then(|c| c["project_id"].as_str())
        .map(str::to_owned);
    let lifetime = response["expires_in"].as_u64().unwrap_or(3600).min(86400);
    let quota = credentials
        .as_ref()
        .and_then(|c| c["quota_project_id"].as_str())
        .map(str::to_owned);
    let fresh = GoogleToken {
        access,
        expires: Instant::now() + Duration::from_secs(lifetime),
        project,
        quota,
    };
    *token = Some(fresh.clone());
    Ok(fresh)
}

async fn endpoint(
    provider: &NativeProvider,
) -> Result<(String, reqwest::header::HeaderMap), ModelError> {
    let mut headers = reqwest::header::HeaderMap::new();
    let base = if provider.name() == "google" {
        let key =
            provider.config.api_key.as_ref().ok_or_else(|| {
                ModelError::Configuration("GEMINI_API_KEY is not configured".into())
            })?;
        headers.insert(
            "x-goog-api-key",
            key.parse()
                .map_err(|_| ModelError::Configuration("invalid Gemini API key header".into()))?,
        );
        provider
            .config
            .base_url
            .clone()
            .or_else(|| env("GOOGLE_API_BASE_URL"))
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".into())
    } else if let Some(key) = provider
        .config
        .api_key
        .clone()
        .or_else(|| env("GOOGLE_CLOUD_API_KEY"))
    {
        headers.insert(
            "x-goog-api-key",
            key.parse()
                .map_err(|_| ModelError::Configuration("invalid Vertex API key header".into()))?,
        );
        provider
            .config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://aiplatform.googleapis.com/v1/publishers/google".into())
    } else {
        let token = access_token(provider).await?;
        headers.insert(
            "authorization",
            format!("Bearer {}", token.access)
                .parse()
                .map_err(|_| ModelError::Configuration("invalid Google token header".into()))?,
        );
        let project = env("GOOGLE_CLOUD_PROJECT")
            .or_else(|| env("GCLOUD_PROJECT"))
            .or(token.project)
            .ok_or_else(|| {
                ModelError::Configuration(
                    "Vertex ADC requires GOOGLE_CLOUD_PROJECT (or GCLOUD_PROJECT)".into(),
                )
            })?;
        let location = env("GOOGLE_CLOUD_LOCATION").unwrap_or_else(|| "us-central1".into());
        if !location
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(ModelError::Configuration(
                "invalid GOOGLE_CLOUD_LOCATION".into(),
            ));
        }
        let host = if location == "global" {
            "aiplatform.googleapis.com".into()
        } else {
            format!("{location}-aiplatform.googleapis.com")
        };
        if let Some(quota) = env("GOOGLE_CLOUD_QUOTA_PROJECT").or(token.quota) {
            headers.insert(
                "x-goog-user-project",
                quota.parse().map_err(|_| {
                    ModelError::Configuration("invalid quota project header".into())
                })?,
            );
        }
        provider.config.base_url.clone().unwrap_or_else(|| {
            format!("https://{host}/v1/projects/{project}/locations/{location}/publishers/google")
        })
    };
    let mut url = reqwest::Url::parse(&format!("{}/", base.trim_end_matches('/')))
        .map_err(|e| ModelError::Configuration(e.to_string()))?;
    url.path_segments_mut()
        .map_err(|()| ModelError::Configuration("invalid Google base URL".into()))?
        .pop_if_empty()
        .push("models")
        .push(&format!(
            "{}:streamGenerateContent",
            provider.model_id().trim_start_matches("models/")
        ));
    url.query_pairs_mut().append_pair("alt", "sse");
    Ok((url.into(), headers))
}

#[cfg(test)]
#[path = "../../../../test/providers/google_auth.rs"]
mod tests;

fn part(part: &ContentPart) -> Value {
    match part {
        ContentPart::Text { text } => json!({"text":text}),
        ContentPart::Image { media_type, data } => {
            json!({"inlineData":{"mimeType":media_type, "data":data}})
        }
    }
}

fn payload(provider: &NativeProvider, request: &ModelRequest) -> Result<Value, ModelError> {
    let names: HashMap<_, _> = request
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .map(|t| (t.id.as_str(), t.function.name.as_str()))
        .collect();
    let mut system = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    for message in &request.messages {
        if message.role == Role::System {
            system.push(json!({"text":message.content}));
            continue;
        }
        let role = if message.role == Role::Assistant {
            "model"
        } else {
            "user"
        };
        let parts = if message.role == Role::Tool {
            let id = message.tool_call_id.as_deref().unwrap_or_default();
            let name = names.get(id).ok_or_else(|| {
                ModelError::Configuration(format!("Google tool result has no matching call: {id}"))
            })?;
            let mut parts = vec![
                json!({"functionResponse":{"id":id, "name":name, "response":{"output":message.content}}}),
            ];
            parts.extend(message.parts.iter().map(part));
            parts
        } else if let Some(saved) = provider.replay(message).and_then(Value::as_array) {
            saved.clone()
        } else {
            let mut parts = Vec::new();
            if !message.content.is_empty() {
                parts.push(json!({"text":message.content}));
            }
            parts.extend(message.parts.iter().map(part));
            for tool in &message.tool_calls {
                let mut part = json!({"functionCall":{"id":tool.id, "name":tool.function.name, "args":arguments(tool)?}});
                // Google's documented escape for imported histories. Never
                // replace a signature on a response produced by this model.
                if provider.model_id().starts_with("gemini-3") {
                    part["thoughtSignature"] = json!("skip_thought_signature_validator");
                }
                parts.push(part);
            }
            parts
        };
        if parts.is_empty() {
            continue;
        }
        if let Some(last) = contents.last_mut().filter(|m| m["role"] == role) {
            last["parts"].as_array_mut().unwrap().extend(parts);
        } else {
            contents.push(json!({"role":role, "parts":parts}));
        }
    }
    let mut body = json!({"contents":contents});
    if !system.is_empty() {
        body["systemInstruction"] = json!({"parts":system});
    }
    if !request.tools.is_empty() {
        body["tools"] = json!([{"functionDeclarations":request.tools.iter().map(|t| json!({"name":t.function.name, "description":t.function.description, "parametersJsonSchema":t.function.parameters})).collect::<Vec<_>>()}]);
    }
    let mut generation = json!({});
    if let Some(max) = provider.config.max_output_tokens {
        generation["maxOutputTokens"] = json!(max);
    }
    if let Some(effort) = provider.config.reasoning_effort {
        generation["thinkingConfig"] = if provider.model_id().starts_with("gemini-3") {
            json!({"includeThoughts":true, "thinkingLevel":if effort == crate::ReasoningEffort::Low { "low" } else { "high" }})
        } else {
            json!({"includeThoughts":true, "thinkingBudget":match effort { crate::ReasoningEffort::Low => 1024, crate::ReasoningEffort::Medium => 8192, _ => 24576 }})
        };
    }
    if generation != json!({}) {
        body["generationConfig"] = generation;
    }
    Ok(body)
}

pub(super) async fn stream(
    provider: &NativeProvider,
    request: ModelRequest,
    delta: &mut (dyn FnMut(String) + Send),
    thinking: &mut (dyn FnMut(String) + Send),
) -> Result<ModelResponse, ModelError> {
    let (url, headers) = endpoint(provider).await?;
    let response = transport::success(
        provider
            .client
            .post(url)
            .headers(headers)
            .json(&payload(provider, &request)?)
            .send()
            .await?,
    )
    .await?;
    let mut output = ModelResponse::default();
    let mut blocks = Vec::new();
    transport::sse(response, |event| {
        if let Some(reason) = event["promptFeedback"]["blockReason"].as_str() {
            return Err(ModelError::InvalidResponse(format!(
                "Google blocked prompt: {reason}"
            )));
        }
        if !event["usageMetadata"].is_null() {
            output.usage = Some(event["usageMetadata"].clone());
        }
        if let Some(candidate) = event["candidates"].as_array().and_then(|c| c.first()) {
            if let Some(reason) = candidate["finishReason"].as_str() {
                if !matches!(reason, "STOP" | "MAX_TOKENS") {
                    return Err(ModelError::InvalidResponse(format!(
                        "Google generation stopped: {reason}"
                    )));
                }
                output.finish_reason = Some(reason.into());
            }
            if let Some(parts) = candidate["content"]["parts"].as_array() {
                for part in parts {
                    if let Some(text) = part["text"].as_str() {
                        if part["thought"] == true {
                            thinking(text.into());
                        } else {
                            output.content.push_str(text);
                            delta(text.into());
                        }
                    }
                    if let Some(function) = part.get("functionCall") {
                        let id = function["id"].as_str().map_or_else(
                            || format!("google_{}", uuid::Uuid::new_v4()),
                            str::to_owned,
                        );
                        output.tool_calls.push(call(
                            id,
                            string(function, "name"),
                            &function["args"],
                        ));
                    }
                    blocks.push(part.clone());
                }
            }
        }
        Ok(())
    })
    .await?;
    if output.finish_reason.is_none() {
        return Err(ModelError::InvalidResponse(
            "Google stream ended without finishReason".into(),
        ));
    }
    // Persist the generated fallback IDs too, so tool result matching survives
    // session reload even when Gemini omitted an ID on the wire.
    let mut calls = output.tool_calls.iter();
    for part in &mut blocks {
        if let Some(function) = part.get_mut("functionCall")
            && let Some(call) = calls.next()
        {
            function["id"] = json!(call.id);
        }
    }
    output.provider_metadata = Some(provider.metadata(json!(blocks)));
    Ok(output)
}

pub(super) async fn models(provider: &NativeProvider) -> Result<Vec<ModelInfo>, ModelError> {
    let key = provider
        .config
        .api_key
        .as_ref()
        .ok_or_else(|| ModelError::Configuration("GEMINI_API_KEY is not configured".into()))?;
    let base = provider
        .config
        .base_url
        .clone()
        .or_else(|| env("GOOGLE_API_BASE_URL"))
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".into());
    let endpoint = format!("{}/models", base.trim_end_matches('/'));
    let mut page = String::new();
    let mut models = Vec::new();
    loop {
        let response: Value = transport::success(
            provider
                .client
                .get(&endpoint)
                .header("x-goog-api-key", key)
                .query(&[("pageToken", &page)])
                .timeout(Duration::from_secs(15))
                .send()
                .await?,
        )
        .await?
        .json()
        .await?;
        let entries = response["models"]
            .as_array()
            .ok_or_else(|| ModelError::InvalidResponse("Google catalog has no models".into()))?;
        for entry in entries {
            if !entry["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|a| a.iter().any(|m| m == "generateContent"))
            {
                continue;
            }
            if let Some(id) = entry["name"].as_str() {
                let mut model = compatible_model_info(
                    id.trim_start_matches("models/").into(),
                    provider.name(),
                    "",
                );
                model.endpoint = None;
                model.display_name = entry["displayName"].as_str().unwrap_or(id).into();
                if let Some(limit) = entry["inputTokenLimit"].as_u64() {
                    model.context_window = usize::try_from(limit).unwrap_or(usize::MAX);
                }
                model.max_output_tokens = entry["outputTokenLimit"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok());
                models.push(model);
            }
        }
        let next = response["nextPageToken"].as_str().unwrap_or_default();
        if next.is_empty() {
            break;
        }
        if next == page {
            return Err(ModelError::InvalidResponse(
                "Google catalog cursor repeated".into(),
            ));
        }
        page = next.into();
    }
    Ok(models)
}

async fn exchange_credentials(
    provider: &NativeProvider,
    credentials: &Value,
    token_endpoint: &str,
) -> Result<reqwest::Response, ModelError> {
    Ok(match credentials["type"].as_str() {
        Some("authorized_user") => {
            provider
                .client
                .post(token_endpoint)
                .form(&[
                    ("grant_type", "refresh_token"),
                    (
                        "client_id",
                        credentials["client_id"].as_str().unwrap_or_default(),
                    ),
                    (
                        "client_secret",
                        credentials["client_secret"].as_str().unwrap_or_default(),
                    ),
                    (
                        "refresh_token",
                        credentials["refresh_token"].as_str().unwrap_or_default(),
                    ),
                ])
                .timeout(Duration::from_secs(20))
                .send()
                .await?
        }
        Some("service_account") => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let key = jsonwebtoken::EncodingKey::from_rsa_pem(
                credentials["private_key"]
                    .as_str()
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .map_err(|e| {
                ModelError::Configuration(format!("invalid Google service account key: {e}"))
            })?;
            let assertion = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256), &json!({"iss":credentials["client_email"], "scope":"https://www.googleapis.com/auth/cloud-platform", "aud":"https://oauth2.googleapis.com/token", "iat":now, "exp":now+3600}), &key).map_err(|e| ModelError::Configuration(e.to_string()))?;
            provider
                .client
                .post(token_endpoint)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                    ("assertion", assertion.as_str()),
                ])
                .timeout(Duration::from_secs(20))
                .send()
                .await?
        }
        kind => {
            return Err(ModelError::Configuration(format!(
                "Google ADC type {kind:?} is not supported; use authorized_user, service_account, a Google-managed identity, or GOOGLE_CLOUD_API_KEY"
            )));
        }
    })
}
