//! Explicit extensions of official Chat Completions endpoints, never inferred
//! from provider/model names. Gateway defaults remain Remote.
use super::{ModelError, OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use crate::search::{self, SearchProtocol, SearchResponse};
use serde_json::{Value, json};

pub(super) fn protocol(config: &OpenAiCompatibleConfig) -> Option<SearchProtocol> {
    if config.api_key.trim().is_empty() || config.api_key_header != "authorization" {
        return None;
    }
    let url = reqwest::Url::parse(&config.endpoint).ok()?;
    let host = url.host_str()?;
    if !search::official_endpoint(&config.endpoint, host) {
        return None;
    }
    match (host, url.path()) {
        ("api.moonshot.ai" | "api.moonshot.cn", "/v1/chat/completions") => {
            Some(SearchProtocol::MoonshotBuiltin)
        }
        ("openrouter.ai", "/api/v1/chat/completions") => Some(SearchProtocol::OpenRouterNative),
        ("api.groq.com", "/openai/v1/chat/completions") => Some(SearchProtocol::GroqBrowserSearch),
        ("api.z.ai" | "open.bigmodel.cn", "/api/paas/v4/chat/completions") => {
            Some(SearchProtocol::ZaiChatSearch)
        }
        // Coding endpoints do not imply the general API's search entitlement.
        // Token Plan's automatic Harness is a different contract. Only the
        // general Model Studio API documents enable_search and source controls.
        (
            "dashscope.aliyuncs.com" | "dashscope-intl.aliyuncs.com",
            "/compatible-mode/v1/chat/completions",
        ) => Some(SearchProtocol::DashscopeSearch),
        (h, "/compatible-mode/v1/chat/completions")
            if h.ends_with(".maas.aliyuncs.com") && !h.starts_with("token-plan.") =>
        {
            Some(SearchProtocol::DashscopeSearch)
        }
        _ => None,
    }
}
impl OpenAiCompatibleProvider {
    #[allow(
        clippy::too_many_lines,
        reason = "Bounded platform search and one Moonshot continuation retain complete receipts"
    )]
    pub(super) async fn native_search_inner(
        &self,
        query: &str,
        protocol: SearchProtocol,
    ) -> Result<SearchResponse, ModelError> {
        let mut body = json!({"model":self.config.model,"messages":[{"role":"user","content":search::prompt(query)}],"stream":false,"max_tokens":2048});
        match protocol {
            SearchProtocol::MoonshotBuiltin => {
                body["tools"] =
                    json!([{"type":"builtin_function","function":{"name":"$web_search"}}]);
            }
            SearchProtocol::OpenRouterNative => {
                body["plugins"] = json!([{"id":"web","engine":"native","max_results":5}]);
            }
            SearchProtocol::ZaiChatSearch => {
                body["tools"] = json!([{"type":"web_search","web_search":{"enable":true,"search_result":true,"search_engine":"search-prime"}}]);
            }
            SearchProtocol::DashscopeSearch => {
                body["enable_search"] = json!(true);
                body["search_options"] = json!({"forced_search":true,"enable_source":true});
            }
            SearchProtocol::GroqBrowserSearch => {
                body["tools"] = json!([{"type":"browser_search"}]);
                body["tool_choice"] = json!("required");
                body["citation_options"] = json!("enabled");
                body["reasoning_effort"] = json!("low");
            }
            _ => {
                return Err(ModelError::Configuration(
                    "wrong Chat search protocol".into(),
                ));
            }
        }
        let mut response = self.search_post(&body).await?;
        let mut rounds = vec![response.clone()];
        if protocol == SearchProtocol::MoonshotBuiltin {
            let choice = &response["choices"][0];
            if choice["finish_reason"] != "tool_calls" {
                return Err(ModelError::InvalidResponse(
                    "Moonshot did not execute builtin search".into(),
                ));
            }
            let calls = choice["message"]["tool_calls"]
                .as_array()
                .filter(|v| v.len() == 1)
                .ok_or_else(|| {
                    ModelError::InvalidResponse("unexpected builtin search calls".into())
                })?;
            let call = &calls[0];
            if call["function"]["name"] != "$web_search" {
                return Err(ModelError::InvalidResponse(
                    "unexpected builtin tool".into(),
                ));
            }
            let arguments = call["function"]["arguments"]
                .as_str()
                .ok_or_else(|| ModelError::InvalidResponse("missing builtin arguments".into()))?;
            let _: Value = serde_json::from_str(arguments)
                .map_err(|_| ModelError::InvalidResponse("invalid builtin arguments".into()))?;
            body["messages"].as_array_mut().unwrap().extend([choice["message"].clone(),json!({"role":"tool","tool_call_id":call["id"],"name":"$web_search","content":arguments})]);
            // A single echo continuation executes the server builtin. Never
            // implement this function locally or repeat an unexpected tool loop.
            response = self.search_post(&body).await?;
            rounds.push(response.clone());
        }
        if response["choices"][0]["finish_reason"] != "stop" {
            return Err(ModelError::InvalidResponse(
                "native Chat search incomplete".into(),
            ));
        }
        let message = &response["choices"][0]["message"];
        let answer = message["content"].as_str().unwrap_or_default().to_owned();
        let mut sources = vec![];
        if protocol == SearchProtocol::GroqBrowserSearch {
            let executed = message["executed_tools"].as_array().ok_or_else(|| {
                ModelError::InvalidResponse("missing server browser search receipt".into())
            })?;
            if !executed
                .iter()
                .any(|t| matches!(t["type"].as_str(), Some("browser_search" | "web_search")))
            {
                return Err(ModelError::InvalidResponse(
                    "no server browser search executed".into(),
                ));
            }
            for tool in executed
                .iter()
                .filter(|t| matches!(t["type"].as_str(), Some("browser_search" | "web_search")))
            {
                for pointer in ["/search_results/results", "/browser_results"] {
                    if let Some(entries) = tool.pointer(pointer).and_then(Value::as_array) {
                        for entry in entries {
                            if let Some(url) = entry["url"].as_str() {
                                sources.extend(search::source(
                                    entry,
                                    url,
                                    entry["title"].as_str().unwrap_or_default(),
                                ));
                            }
                        }
                    }
                }
            }
        }
        if let Some(annotations) = message["annotations"].as_array() {
            for entry in annotations {
                let s = entry.get("url_citation").unwrap_or(entry);
                if let Some(url) = s["url"].as_str() {
                    sources.extend(search::source(
                        entry,
                        url,
                        s["title"].as_str().unwrap_or_default(),
                    ));
                }
            }
        }
        for pointer in [
            "/web_search",
            "/search_info/search_results",
            "/choices/0/message/search_info/search_results",
        ] {
            if let Some(entries) = response.pointer(pointer).and_then(Value::as_array) {
                for entry in entries {
                    if let Some(url) = entry["link"].as_str().or_else(|| entry["url"].as_str()) {
                        sources.extend(search::source(
                            entry,
                            url,
                            entry["title"].as_str().unwrap_or_default(),
                        ));
                    }
                }
            }
        }
        if protocol == SearchProtocol::MoonshotBuiltin && sources.is_empty() {
            // Moonshot exposes its final references as Markdown, not OpenAI
            // annotation objects. Keep these as model-provided references and
            // preserve the actual builtin receipt for auditability.
            for tail in answer.split("](").skip(1) {
                if let Some((url, _)) = tail.split_once(')') {
                    sources.extend(search::source(
                        &json!({"kind":"builtin_answer_reference"}),
                        url,
                        url,
                    ));
                }
            }
        }
        search::validated(SearchResponse {
            answer,
            sources,
            evidence: json!({"rounds":rounds}),
        })
    }
    async fn search_post(&self, body: &Value) -> Result<Value, ModelError> {
        search::json(
            self.client
                .post(&self.config.endpoint)
                .header(
                    &self.config.api_key_header,
                    format!("Bearer {}", self.config.api_key),
                )
                .json(body)
                .send()
                .await?,
        )
        .await
    }
}
