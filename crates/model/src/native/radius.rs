use super::{NativeProvider, arguments, call, string, transport};
use crate::ModelProvider;
use crate::{
    ContentPart, ModelError, ModelInfo, ModelRequest, ModelResponse, Role,
    providers::compatible_model_info,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn base(provider: &NativeProvider) -> String {
    provider
        .config
        .base_url
        .clone()
        .or_else(|| std::env::var("RADIUS_BASE_URL").ok())
        .unwrap_or_else(|| "https://radius.pi.dev".into())
        .trim_end_matches('/')
        .into()
}
fn key(provider: &NativeProvider) -> Result<&str, ModelError> {
    provider
        .config
        .api_key
        .as_deref()
        .ok_or_else(|| ModelError::Configuration("RADIUS_API_KEY is not configured".into()))
}
fn parts(message: &crate::Message) -> Vec<Value> {
    let mut content = Vec::new();
    if !message.content.is_empty() {
        content.push(json!({"type":"text", "text":message.content}));
    }
    content.extend(message.parts.iter().map(|p| match p {
        ContentPart::Text { text } => json!({"type":"text", "text":text}),
        ContentPart::Image { media_type, data } => {
            json!({"type":"image", "mimeType":media_type, "data":data})
        }
    }));
    content
}
fn payload(provider: &NativeProvider, request: &ModelRequest) -> Result<Value, ModelError> {
    let timestamp = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX);
    let tools: Vec<_> = request.tools.iter().map(|t| json!({"name":t.function.name, "description":t.function.description, "parameters":t.function.parameters})).collect();
    let names: HashMap<_, _> = request
        .messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .map(|c| (c.id.as_str(), c.function.name.as_str()))
        .collect();
    let mut messages = Vec::new();
    for message in &request.messages {
        let converted = match message.role {
            Role::System => {
                json!({"role":"system", "content":message.content, "timestamp":timestamp})
            }
            Role::User => json!({"role":"user", "content":parts(message), "timestamp":timestamp}),
            Role::Assistant => {
                let content = if let Some(saved) = provider.replay(message) {
                    saved.clone()
                } else {
                    let mut content = parts(message);
                    for tool in &message.tool_calls {
                        content.push(json!({"type":"toolCall", "id":tool.id, "name":tool.function.name, "arguments":arguments(tool)?}));
                    }
                    json!(content)
                };
                json!({"role":"assistant", "content":content, "api":"pi-messages", "provider":"radius", "model":provider.model_id(), "stopReason":if message.tool_calls.is_empty() { "stop" } else { "toolUse" }, "usage":message.usage.as_ref().and_then(|u| u.get("reported")).cloned().unwrap_or_else(|| json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}})), "timestamp":timestamp})
            }
            Role::Tool => {
                let id = message.tool_call_id.as_deref().unwrap_or_default();
                let name = names.get(id).ok_or_else(|| {
                    ModelError::Configuration(format!("Radius tool result has no call: {id}"))
                })?;
                json!({"role":"toolResult", "toolCallId":id, "toolName":name, "content":parts(message), "isError":false, "timestamp":timestamp})
            }
        };
        messages.push(converted);
    }
    if messages.first().is_none_or(|m| m["role"] != "system") {
        messages.insert(
            0,
            json!({"role":"system", "content":"", "timestamp":timestamp}),
        );
    }
    if !tools.is_empty() {
        messages[0]["toolsAdded"] = json!(tools);
    }
    let mut options = json!({});
    if let Some(max) = provider.config.max_output_tokens {
        options["maxTokens"] = json!(max);
    }
    if let Some(effort) = provider.config.reasoning_effort {
        options["reasoning"] = json!(effort.to_string());
    }
    Ok(json!({"model":provider.model_id(), "context":{"messages":messages}, "options":options}))
}

pub(super) async fn stream(
    provider: &NativeProvider,
    request: ModelRequest,
    delta: &mut (dyn FnMut(String) + Send),
    thinking: &mut (dyn FnMut(String) + Send),
) -> Result<ModelResponse, ModelError> {
    let inference_base = match &provider.config.base_url {
        Some(base) => base.clone(),
        None => provider
            .radius_base
            .get_or_try_init(|| async {
                let models = models(provider).await?;
                models
                    .into_iter()
                    .find(|model| model.id == provider.model_id())
                    .and_then(|model| model.endpoint)
                    .ok_or_else(|| {
                        ModelError::Configuration(format!(
                            "Radius gateway did not advertise model {}",
                            provider.model_id()
                        ))
                    })
            })
            .await?
            .clone(),
    };
    let response = transport::success(
        provider
            .client
            .post(format!("{}/messages", inference_base.trim_end_matches('/')))
            .bearer_auth(key(provider)?)
            .header("accept", "text/event-stream")
            .json(&payload(provider, &request)?)
            .send()
            .await?,
    )
    .await?;
    let mut blocks = BTreeMap::<usize, Value>::new();
    let mut output = ModelResponse::default();
    transport::sse(response, |event| {
        let index = super::index(&event, "contentIndex")?;
        let kind = event["type"].as_str().unwrap_or_default();
        match kind {
            "text_start" => { blocks.insert(index, json!({"type":"text", "text":""})); }
            "thinking_start" => { blocks.insert(index, json!({"type":"thinking", "thinking":""})); }
            "text_delta" | "thinking_delta" => {
                let text = string(&event, "delta");
                if kind == "text_delta" { delta(text.clone()); } else { thinking(text.clone()); }
                let field = if kind == "text_delta" { "text" } else { "thinking" };
                let block = blocks.get_mut(&index).ok_or_else(|| ModelError::InvalidResponse("Radius delta has no block".into()))?;
                block[field] = json!(format!("{}{}", block[field].as_str().unwrap_or_default(), text));
            }
            "text_end" | "thinking_end" => {
                let block = blocks.get_mut(&index).ok_or_else(|| ModelError::InvalidResponse("Radius end has no block".into()))?;
                let (field, signature) = if kind == "text_end" { ("text", "textSignature") } else { ("thinking", "thinkingSignature") };
                block[field] = event["content"].clone();
                if !event["contentSignature"].is_null() { block[signature] = event["contentSignature"].clone(); }
                if !event["redacted"].is_null() { block["redacted"] = event["redacted"].clone(); }
            }
            "toolcall_start" => { blocks.insert(index, json!({"type":"toolCall", "id":event["id"], "name":event["toolName"], "arguments":{}})); }
            // toolcall_end contains the final parsed arguments, including a
            // provider signature; do not try to execute incomplete JSON.
            "toolcall_end" => { blocks.insert(index, event["toolCall"].clone()); }
            "done" => {
                if !matches!(event["reason"].as_str(), Some("stop" | "length" | "toolUse")) { return Err(ModelError::InvalidResponse("invalid Radius terminal reason".into())); }
                output.finish_reason = event["reason"].as_str().map(str::to_owned); output.usage = Some(event["usage"].clone());
            }
            _ => {}
        }
        Ok(())
    }).await?;
    if output.finish_reason.is_none() {
        return Err(ModelError::InvalidResponse(
            "Radius stream ended without done".into(),
        ));
    }
    let blocks: Vec<_> = blocks.into_values().collect();
    for block in &blocks {
        match block["type"].as_str() {
            Some("text") => output.content.push_str(&string(block, "text")),
            Some("toolCall") => output.tool_calls.push(call(
                string(block, "id"),
                string(block, "name"),
                &block["arguments"],
            )),
            _ => {}
        }
    }
    output.provider_metadata = Some(provider.metadata(json!(blocks)));
    Ok(output)
}

pub(super) async fn models(provider: &NativeProvider) -> Result<Vec<ModelInfo>, ModelError> {
    let response: Value = transport::success(
        provider
            .client
            .get(format!("{}/v1/config", base(provider)))
            .bearer_auth(key(provider)?)
            .timeout(Duration::from_secs(15))
            .send()
            .await?,
    )
    .await?
    .json()
    .await?;
    let endpoint = response["baseUrl"]
        .as_str()
        .ok_or_else(|| ModelError::InvalidResponse("Radius config has no baseUrl".into()))?;
    let url =
        reqwest::Url::parse(endpoint).map_err(|e| ModelError::InvalidResponse(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ModelError::InvalidResponse(
            "Radius config baseUrl is not HTTP".into(),
        ));
    }
    let entries = response["models"]
        .as_array()
        .ok_or_else(|| ModelError::InvalidResponse("Radius config has no models".into()))?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let id = entry["id"].as_str()?;
            let mut model = compatible_model_info(id.into(), provider.name(), endpoint);
            model.display_name = entry["name"].as_str().unwrap_or(id).into();
            model.context_window = entry["contextWindow"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(128_000);
            model.max_output_tokens = entry["maxTokens"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok());
            Some(model)
        })
        .collect())
}
