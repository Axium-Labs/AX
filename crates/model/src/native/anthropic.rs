use super::{NativeProvider, arguments, call, string, transport};
use crate::ModelProvider;
use crate::{
    ContentPart, ModelError, ModelInfo, ModelRequest, ModelResponse, Role,
    providers::compatible_model_info,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn base(provider: &NativeProvider) -> String {
    provider
        .config
        .base_url
        .clone()
        .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
        .unwrap_or_else(|| "https://api.anthropic.com".into())
        .trim_end_matches('/')
        .to_owned()
}

pub(super) fn search_supported(provider: &NativeProvider) -> bool {
    crate::search::official_endpoint(&base(provider), "api.anthropic.com")
        && reqwest::Url::parse(&base(provider)).is_ok_and(|u| matches!(u.path(), "" | "/" | "/v1"))
        && provider
            .config
            .api_key
            .as_ref()
            .is_some_and(|k| !k.trim().is_empty())
}

#[allow(
    clippy::too_many_lines,
    reason = "One bounded server stream accumulates tool arguments, citations and terminal evidence"
)]
pub(super) async fn search(
    provider: &NativeProvider,
    query: &str,
) -> Result<crate::search::SearchResponse, ModelError> {
    let base = base(provider);
    let endpoint = if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    };
    let body = json!({"model":provider.model_id(),"max_tokens":2048,"stream":true,
        "messages":[{"role":"user","content":crate::search::prompt(query)}],
        "tools":[{"type":"web_search_20250305","name":"web_search","max_uses":1}],
        "tool_choice":{"type":"tool","name":"web_search"}});
    let http = authenticated(provider, provider.client.post(endpoint))?
        .json(&body)
        .send()
        .await?;
    let mut blocks = BTreeMap::<usize, Value>::new();
    let mut stop = None;
    let mut usage = json!({});
    let mut finished = false;
    crate::search::stream(http, |event| {
        match event["type"].as_str() {
            Some("message_start") => usage = event["message"]["usage"].clone(),
            Some("content_block_start") => {
                blocks.insert(
                    super::index(&event, "index")?,
                    event["content_block"].clone(),
                );
            }
            Some("content_block_delta") => {
                let block = blocks
                    .get_mut(&super::index(&event, "index")?)
                    .ok_or_else(|| {
                        ModelError::InvalidResponse("search delta without block".into())
                    })?;
                let delta = &event["delta"];
                if delta["type"] == "text_delta" {
                    let text = format!(
                        "{}{}",
                        block["text"].as_str().unwrap_or_default(),
                        delta["text"].as_str().unwrap_or_default()
                    );
                    block["text"] = json!(text);
                } else if delta["type"] == "citations_delta" {
                    if !block["citations"].is_array() {
                        block["citations"] = json!([]);
                    }
                    block["citations"]
                        .as_array_mut()
                        .unwrap()
                        .push(delta["citation"].clone());
                } else if delta["type"] == "input_json_delta" {
                    block["partial_json"] = json!(format!(
                        "{}{}",
                        block["partial_json"].as_str().unwrap_or_default(),
                        delta["partial_json"].as_str().unwrap_or_default()
                    ));
                }
            }
            Some("content_block_stop") => {
                if let Some(block) = blocks.get_mut(&super::index(&event, "index")?)
                    && let Some(partial) = block["partial_json"].as_str()
                {
                    block["input"] = serde_json::from_str(partial).map_err(|_| {
                        ModelError::InvalidResponse("invalid server tool arguments".into())
                    })?;
                    block.as_object_mut().unwrap().remove("partial_json");
                }
            }
            Some("message_delta") => {
                stop = event["delta"]["stop_reason"].as_str().map(str::to_owned);
                usage["output"] = event["usage"].clone();
            }
            Some("message_stop") => finished = true,
            _ => {}
        }
        Ok(())
    })
    .await?;
    if !finished || stop.as_deref() != Some("end_turn") {
        return Err(ModelError::InvalidResponse(
            "native Messages search incomplete or paused".into(),
        ));
    }
    let content = blocks.into_values().collect::<Vec<_>>();
    let mut sources = vec![];
    let mut answer = String::new();
    let mut executed = false;
    for block in &content {
        if block["type"] == "text" {
            answer.push_str(block["text"].as_str().unwrap_or_default());
        }
        if block["type"] == "web_search_tool_result" {
            let entries = block["content"].as_array().ok_or_else(|| {
                ModelError::InvalidResponse("server web search returned tool error".into())
            })?;
            executed = true;
            sources.extend(entries.iter().filter_map(|s| {
                crate::search::source(
                    s,
                    s["url"].as_str()?,
                    s["title"].as_str().unwrap_or_default(),
                )
            }));
        }
    }
    if !executed {
        return Err(ModelError::InvalidResponse(
            "no server web search executed".into(),
        ));
    }
    crate::search::validated(crate::search::SearchResponse {
        answer,
        sources,
        evidence: json!({"content":content,"usage":usage}),
    })
}
fn authenticated(
    provider: &NativeProvider,
    request: reqwest::RequestBuilder,
) -> Result<reqwest::RequestBuilder, ModelError> {
    let key =
        provider.config.api_key.as_ref().ok_or_else(|| {
            ModelError::Configuration("ANTHROPIC_API_KEY is not configured".into())
        })?;
    Ok(request
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01"))
}

pub(super) fn payload(
    provider: &NativeProvider,
    request: &ModelRequest,
) -> Result<Value, ModelError> {
    let mut system = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        if message.role == Role::System {
            system.push(json!({"type":"text", "text":message.content}));
            continue;
        }
        let role = if message.role == Role::Assistant {
            "assistant"
        } else {
            "user"
        };
        let mut content = Vec::new();
        if message.role == Role::Tool {
            let mut result = vec![json!({"type":"text", "text":message.content})];
            result.extend(parts(&message.parts));
            content.push(
                json!({"type":"tool_result", "tool_use_id":message.tool_call_id, "content":result}),
            );
        } else if let Some(saved) = provider.replay(message).and_then(Value::as_array) {
            content.clone_from(saved);
        } else {
            if !message.content.is_empty() {
                content.push(json!({"type":"text", "text":message.content}));
            }
            content.extend(parts(&message.parts));
            for tool in &message.tool_calls {
                content.push(json!({"type":"tool_use", "id":tool.id, "name":tool.function.name, "input":arguments(tool)?}));
            }
        }
        if content.is_empty() {
            continue;
        }
        // Anthropic expects alternating roles; multiple parallel tool results
        // belong in one user message rather than consecutive user turns.
        if let Some(previous) = messages.last_mut().filter(|m| m["role"] == role) {
            previous["content"].as_array_mut().unwrap().extend(content);
        } else {
            messages.push(json!({"role":role, "content":content}));
        }
    }
    let tools: Vec<_> = request.tools.iter().map(|t| json!({"name":t.function.name, "description":t.function.description, "input_schema":t.function.parameters})).collect();
    let mut body = json!({"model":provider.model_id(), "system":system, "messages":messages, "max_tokens":provider.config.max_output_tokens.unwrap_or(8192), "stream":true});
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    // Extended thinking is opt-in. Use a bounded budget on models with the
    // older budget API; newer adaptive models accept adaptive + output effort.
    if let Some(effort) = provider.config.reasoning_effort {
        if super::adaptive_thinking(provider.model_id()) {
            body["thinking"] = json!({"type":"adaptive"});
            body["output_config"] = json!({"effort":super::adaptive_effort(effort)});
        } else {
            let maximum = body["max_tokens"].as_u64().unwrap_or(8192);
            if maximum > 1024 {
                body["thinking"] = json!({"type":"enabled", "budget_tokens": match effort { crate::ReasoningEffort::Low => 1024, crate::ReasoningEffort::Medium => 4096, _ => 8192 }.min(maximum - 1)});
            }
        }
    }
    Ok(body)
}

fn parts(parts: &[ContentPart]) -> Vec<Value> {
    parts.iter().map(|part| match part {
        ContentPart::Text { text } => json!({"type":"text", "text":text}),
        ContentPart::Image { media_type, data } => json!({"type":"image", "source":{"type":"base64", "media_type":media_type, "data":data}}),
    }).collect()
}

pub(super) async fn stream(
    provider: &NativeProvider,
    request: ModelRequest,
    delta: &mut (dyn FnMut(String) + Send),
    thinking: &mut (dyn FnMut(String) + Send),
) -> Result<ModelResponse, ModelError> {
    let endpoint = format!("{}/v1/messages", base(provider).trim_end_matches("/v1"));
    let response = transport::success(
        authenticated(provider, provider.client.post(endpoint))?
            .json(&payload(provider, &request)?)
            .send()
            .await?,
    )
    .await?;
    let mut output = ModelResponse::default();
    let mut blocks = BTreeMap::<usize, Value>::new();
    let mut inputs = BTreeMap::<usize, String>::new();
    let mut done = false;
    transport::sse(response, |event| {
        let index = super::index(&event, "index")?;
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                output.usage = Some(event["message"]["usage"].clone());
            }
            "content_block_start" => {
                blocks.insert(index, event["content_block"].clone());
            }
            "content_block_delta" => {
                let change = &event["delta"];
                let block = blocks.get_mut(&index).ok_or_else(|| {
                    ModelError::InvalidResponse("Anthropic delta has no content block".into())
                })?;
                let (field, fragment) = match change["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let text = string(change, "text");
                        delta(text.clone());
                        ("text", text)
                    }
                    "thinking_delta" => {
                        let text = string(change, "thinking");
                        thinking(text.clone());
                        ("thinking", text)
                    }
                    "signature_delta" => ("signature", string(change, "signature")),
                    "input_json_delta" => {
                        inputs
                            .entry(index)
                            .or_default()
                            .push_str(&string(change, "partial_json"));
                        return Ok(());
                    }
                    _ => return Ok(()),
                };
                block[field] = json!(format!(
                    "{}{}",
                    block[field].as_str().unwrap_or_default(),
                    fragment
                ));
            }
            "content_block_stop" => {
                if let Some(input) = inputs.remove(&index) {
                    blocks.get_mut(&index).unwrap()["input"] = serde_json::from_str(&input)
                        .map_err(|e| ModelError::InvalidResponse(e.to_string()))?;
                }
            }
            "message_delta" => {
                output.finish_reason = event["delta"]["stop_reason"].as_str().map(str::to_owned);
                if let Some(usage) = event["usage"].as_object() {
                    for (name, value) in usage {
                        output.usage.get_or_insert_with(|| json!({}))[name] = value.clone();
                    }
                }
            }
            "message_stop" => done = true,
            _ => {}
        }
        Ok(())
    })
    .await?;
    if !done || !inputs.is_empty() {
        return Err(ModelError::InvalidResponse(
            "Anthropic stream ended before message_stop".into(),
        ));
    }
    let blocks: Vec<_> = blocks.into_values().collect();
    for block in &blocks {
        match block["type"].as_str() {
            Some("text") => output.content.push_str(&string(block, "text")),
            Some("tool_use") => output.tool_calls.push(call(
                string(block, "id"),
                string(block, "name"),
                &block["input"],
            )),
            _ => {}
        }
    }
    output.provider_metadata = Some(provider.metadata(json!(blocks)));
    Ok(output)
}

pub(super) async fn models(provider: &NativeProvider) -> Result<Vec<ModelInfo>, ModelError> {
    let endpoint = format!("{}/v1/models", base(provider).trim_end_matches("/v1"));
    let mut models = Vec::new();
    let mut after = None;
    loop {
        let mut request = authenticated(provider, provider.client.get(&endpoint))?
            .timeout(std::time::Duration::from_secs(15));
        if let Some(cursor) = &after {
            request = request.query(&[("after_id", cursor)]);
        }
        let response: Value = transport::success(request.send().await?)
            .await?
            .json()
            .await?;
        let data = response["data"]
            .as_array()
            .ok_or_else(|| ModelError::InvalidResponse("Anthropic catalog has no data".into()))?;
        for entry in data {
            if let Some(id) = entry["id"].as_str() {
                let mut model = compatible_model_info(id.into(), provider.name(), &endpoint);
                model.endpoint = None;
                model.display_name = entry["display_name"].as_str().unwrap_or(id).into();
                models.push(model);
            }
        }
        if response["has_more"] != true {
            break;
        }
        let next = response["last_id"]
            .as_str()
            .ok_or_else(|| ModelError::InvalidResponse("Anthropic catalog has no cursor".into()))?
            .to_owned();
        if after.as_ref() == Some(&next) {
            return Err(ModelError::InvalidResponse(
                "Anthropic catalog cursor repeated".into(),
            ));
        }
        after = Some(next);
    }
    Ok(models)
}
