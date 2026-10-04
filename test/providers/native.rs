//! Local wire-level regressions; no credentials, billing, or internet required.
use model::{
    ContentPart, ErrorClass, FunctionSpec, Message, ModelError, ModelProvider, ModelRequest,
    NativeConfig, NativeProvider, OpenAiConfig, OpenAiProvider, ToolSpec,
    provider_adapter_with_client,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt::Write as _, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

struct Recorded {
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}
struct Reply {
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
    extra: &'static str,
}
impl Reply {
    fn sse(events: Vec<Value>) -> Self {
        Self {
            status: 200,
            mime: "text/event-stream",
            extra: "",
            body: events
                .into_iter()
                .fold(String::new(), |mut text, event| {
                    write!(&mut text, "data: {event}\r\n\r\n").unwrap();
                    text
                })
                .into_bytes(),
        }
    }
    fn json(body: &Value) -> Self {
        Self {
            status: 200,
            mime: "application/json",
            extra: "",
            body: body.to_string().into_bytes(),
        }
    }
}
async fn mock(replies: Vec<Reply>) -> (String, JoinHandle<Vec<Recorded>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 4096];
            let split = loop {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let head = String::from_utf8(bytes[..split].to_vec()).unwrap();
            let mut lines = head.lines();
            let path = lines
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .into();
            let headers: BTreeMap<_, _> = lines
                .filter_map(|line| {
                    line.split_once(':')
                        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
                })
                .collect();
            let length = headers
                .get("content-length")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            while bytes.len() < split + length {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            let body = if length == 0 {
                Value::Null
            } else {
                serde_json::from_slice(&bytes[split..split + length]).unwrap()
            };
            requests.push(Recorded {
                path,
                headers,
                body,
            });
            socket.write_all(format!("HTTP/1.1 {} Test\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n", reply.status, reply.mime, reply.body.len(), reply.extra).as_bytes()).await.unwrap();
            // Deliberately fragment frames and Unicode across transport writes.
            for chunk in reply.body.chunks(3) {
                socket.write_all(chunk).await.unwrap();
                tokio::task::yield_now().await;
            }
        }
        requests
    });
    (base, task)
}
fn tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "read_file".into(),
            description: "Read".into(),
            parameters: json!({"type":"object", "properties":{"path":{"type":"string"}}, "required":["path"]}),
        },
    }]
}
fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        messages,
        tools: tools(),
    }
}
fn config(id: &str, base: &str) -> NativeConfig {
    let mut config = NativeConfig::new(id, "test-model".into(), Some("test-key".into()), 128_000);
    config.base_url = Some(base.into());
    config.region = Some("us-east-1".into());
    config
}
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
fn native(config: NativeConfig) -> Result<NativeProvider, ModelError> {
    NativeProvider::with_client(config, client())
}
fn restored(response: &model::ModelResponse) -> Message {
    let mut message = Message::assistant(response.content.clone(), response.tool_calls.clone());
    message
        .provider_metadata
        .clone_from(&response.provider_metadata);
    serde_json::from_value(serde_json::to_value(message).unwrap()).unwrap()
}
fn anthropic_tool() -> Reply {
    Reply::sse(vec![
        json!({"type":"message_start", "message":{"usage":{"input_tokens":10,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"思考"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed-thought"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-a","name":"read_file","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"代码.rs\"}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}),
        json!({"type":"message_stop"}),
    ])
}
fn anthropic_text() -> Reply {
    Reply::sse(vec![
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"完成 ✓"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ])
}

#[tokio::test]
async fn anthropic_signed_thinking_tool_arguments_and_history_survive_reload() {
    let (base, server) = mock(vec![anthropic_tool(), anthropic_text()]).await;
    let provider = native(config("anthropic", &base)).unwrap();
    let messages = vec![Message::system("rules"), Message::user("read")];
    let mut thinking = String::new();
    let first = provider
        .complete_stream(request(messages.clone()), &mut |_| {}, &mut |s| {
            thinking.push_str(&s);
        })
        .await
        .unwrap();
    assert_eq!(thinking, "思考");
    assert_eq!(
        first.tool_calls[0].function.arguments,
        "{\"path\":\"代码.rs\"}"
    );
    assert_eq!(first.usage.as_ref().unwrap()["input_tokens"], 10);
    assert_eq!(first.usage.as_ref().unwrap()["output_tokens"], 5);
    let mut next = messages;
    next.push(restored(&first));
    next.push(Message::tool("call-a", "content"));
    let mut text = String::new();
    let second = provider
        .complete_stream(request(next), &mut |s| text.push_str(&s), &mut |_| {})
        .await
        .unwrap();
    assert_eq!(second.content, "完成 ✓");
    assert_eq!(text, second.content);
    let requests = server.await.unwrap();
    assert_eq!(requests[0].path, "/v1/messages");
    assert_eq!(requests[0].headers["x-api-key"], "test-key");
    assert_eq!(requests[0].headers["anthropic-version"], "2023-06-01");
    assert!(!requests[0].headers.contains_key("authorization"));
    assert_eq!(
        requests[0].body["tools"][0]["input_schema"]["type"],
        "object"
    );
    assert_eq!(
        requests[1].body["messages"][1]["content"][0]["signature"],
        "signed-thought"
    );
    assert_eq!(
        requests[1].body["messages"][2]["content"][0]["tool_use_id"],
        "call-a"
    );
}

#[tokio::test]
async fn google_and_vertex_express_replay_function_signature_and_images() {
    for id in ["google", "google-vertex"] {
        let tool = json!({"candidates":[{"content":{"parts":[{"text":"思考", "thought":true}, {"functionCall":{"name":"read_file", "args":{"path":"a.rs"}}, "thoughtSignature":"gemini-signature"}]},"finishReason":"STOP"}], "usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}});
        let text =
            json!({"candidates":[{"content":{"parts":[{"text":"完成"}]},"finishReason":"STOP"}]});
        let (base, server) = mock(vec![Reply::sse(vec![tool]), Reply::sse(vec![text])]).await;
        let provider = native(config(id, &base)).unwrap();
        let mut user = Message::user("read");
        user.parts.push(ContentPart::Image {
            media_type: "image/png".into(),
            data: "AQID".into(),
        });
        let messages = vec![Message::system("rules"), user];
        let first = provider.complete(request(messages.clone())).await.unwrap();
        assert_eq!(first.tool_calls.len(), 1);
        let mut next = messages;
        next.push(restored(&first));
        next.push(Message::tool(&first.tool_calls[0].id, "code"));
        let second = provider.complete(request(next)).await.unwrap();
        assert_eq!(second.content, "完成");
        let requests = server.await.unwrap();
        assert_eq!(requests[0].headers["x-goog-api-key"], "test-key");
        assert!(
            requests[0]
                .path
                .starts_with("/models/test-model:streamGenerateContent?alt=sse")
        );
        assert_eq!(
            requests[0].body["contents"][0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(
            requests[1].body["contents"][1]["parts"][1]["thoughtSignature"],
            "gemini-signature"
        );
        assert_eq!(
            requests[1].body["contents"][2]["parts"][0]["functionResponse"]["name"],
            "read_file"
        );
    }
}

#[tokio::test]
async fn google_catalog_pages_filter_embedding_and_preserve_limits() {
    let (base, server) = mock(vec![
        Reply::json(&json!({"models":[{"name":"models/gemini-test", "displayName":"Gemini Test", "inputTokenLimit":1_000_000,"outputTokenLimit":8192,"supportedGenerationMethods":["generateContent"]},{"name":"models/embed", "supportedGenerationMethods":["embedContent"]}],"nextPageToken":"next"})),
        Reply::json(&json!({"models":[{"name":"models/gemini-other", "supportedGenerationMethods":["generateContent"]}]})),
    ]).await;
    let provider = native(config("google", &base)).unwrap();
    let models = provider.list_models().await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].context_window, 1_000_000);
    assert_eq!(models[0].max_output_tokens, Some(8192));
    assert!(server.await.unwrap()[1].path.contains("pageToken=next"));
}

#[tokio::test]
async fn radius_uses_pi_messages_and_dynamic_config_instead_of_openai() {
    let events = vec![
        json!({"type":"toolcall_start", "contentIndex":0, "id":"radius-call", "toolName":"read_file"}),
        json!({"type":"toolcall_delta", "contentIndex":0, "delta":"{\"path\":\"a\"}"}),
        json!({"type":"toolcall_end", "contentIndex":0,"toolCall":{"type":"toolCall","id":"radius-call","name":"read_file","arguments":{"path":"a"}, "thoughtSignature":"radius-sig"}}),
        json!({"type":"done", "reason":"toolUse", "usage":{"input":10,"output":5}}),
    ];
    let (base, server) = mock(vec![Reply::json(&json!({"baseUrl":"https://radius.pi.dev/inference", "models":[{"id":"radius-model","name":"Radius Model","contextWindow":200_000,"maxTokens":8000}]})),Reply::sse(events)]).await;
    let provider = native(config("radius", &base)).unwrap();
    let models = provider.list_models().await.unwrap();
    assert_eq!(
        models[0].endpoint.as_deref(),
        Some("https://radius.pi.dev/inference")
    );
    let response = provider
        .complete(request(vec![
            Message::system("rules"),
            Message::user("read"),
        ]))
        .await
        .unwrap();
    assert_eq!(response.tool_calls[0].id, "radius-call");
    let requests = server.await.unwrap();
    assert_eq!(requests[0].path, "/v1/config");
    assert_eq!(requests[1].path, "/messages");
    assert_eq!(
        requests[1].body["context"]["messages"][0]["toolsAdded"][0]["name"],
        "read_file"
    );
    assert_eq!(
        response.provider_metadata.unwrap()["content"][0]["thoughtSignature"],
        "radius-sig"
    );
}

fn aws_frame(kind: &str, body: &Value) -> Vec<u8> {
    use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
    let message = Message::new_from_parts(
        vec![
            Header::new(":message-type", HeaderValue::String("event".into())),
            Header::new(":event-type", HeaderValue::String(kind.to_owned().into())),
        ],
        body.to_string().into_bytes(),
    );
    let mut bytes = Vec::new();
    aws_smithy_eventstream::frame::write_message_to(&message, &mut bytes).unwrap();
    bytes
}
fn bedrock_reply() -> Reply {
    let mut body = Vec::new();
    for (kind, data) in [
        ("messageStart", json!({"role":"assistant"})),
        (
            "contentBlockStart",
            json!({"contentBlockIndex":0,"start":{"toolUse":{"toolUseId":"aws-call","name":"read_file"}}}),
        ),
        (
            "contentBlockDelta",
            json!({"contentBlockIndex":0,"delta":{"toolUse":{"input":"{\"path\":\"a.rs\"}"}}}),
        ),
        ("contentBlockStop", json!({"contentBlockIndex":0})),
        ("messageStop", json!({"stopReason":"tool_use"})),
        (
            "metadata",
            json!({"usage":{"inputTokens":10,"outputTokens":5}}),
        ),
    ] {
        body.extend(aws_frame(kind, &data));
    }
    Reply {
        status: 200,
        mime: "application/vnd.amazon.eventstream",
        body,
        extra: "",
    }
}
#[tokio::test]
async fn bedrock_bearer_and_sigv4_parse_fragmented_binary_stream() {
    for bearer in [true, false] {
        let (base, server) = mock(vec![bedrock_reply()]).await;
        let mut config = config("amazon-bedrock", &base);
        config.model = "us.anthropic.claude-test-v1:0".into();
        if !bearer {
            config.api_key = None;
            config.aws_credentials = Some(aws_credential_types::Credentials::new(
                "AKIDEXAMPLE",
                "secret-example",
                Some("session-token".into()),
                None,
                "test",
            ));
        }
        let provider = native(config).unwrap();
        let response = provider
            .complete(request(vec![
                Message::system("rules"),
                Message::user("read"),
            ]))
            .await
            .unwrap();
        assert_eq!(
            response.tool_calls[0].function.arguments,
            "{\"path\":\"a.rs\"}"
        );
        assert_eq!(response.usage.unwrap()["inputTokens"], 10);
        let requests = server.await.unwrap();
        assert!(requests[0].path.ends_with("/converse-stream"));
        assert_eq!(
            requests[0].body["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["type"],
            "object"
        );
        if bearer {
            assert_eq!(requests[0].headers["authorization"], "Bearer test-key");
        } else {
            let auth = &requests[0].headers["authorization"];
            assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
            assert!(auth.contains("/us-east-1/bedrock/aws4_request"));
            assert_eq!(requests[0].headers["x-amz-security-token"], "session-token");
            assert!(requests[0].headers.contains_key("x-amz-date"));
        }
    }
}

#[tokio::test]
async fn cloudflare_gateway_uses_gateway_header_without_bearer_authorization() {
    let (base, server) = mock(vec![Reply::json(
        &json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}),
    )])
    .await;
    let mut config = config("cloudflare-ai-gateway", &base);
    config.base_url = Some(format!("{base}/compat/chat/completions"));
    let provider = provider_adapter_with_client(config, client()).unwrap();
    assert_eq!(provider.name(), "cloudflare-ai-gateway");
    assert_eq!(
        provider
            .complete(request(vec![Message::user("hi")]))
            .await
            .unwrap()
            .content,
        "ok"
    );
    let requests = server.await.unwrap();
    assert_eq!(
        requests[0].headers["cf-aig-authorization"],
        "Bearer test-key"
    );
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn azure_responses_preserves_identity_and_uses_deployment_and_api_key() {
    let (base, server) = mock(vec![Reply::json(&json!({"output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]}],"status":"completed"}))]).await;
    let config = OpenAiConfig::from_azure(
        "gpt-test".into(),
        "azure-key".into(),
        format!("{base}/openai/v1/responses"),
        Some("my-deployment".into()),
    )
    .unwrap();
    let provider = OpenAiProvider::with_client(config, client());
    assert_eq!(provider.name(), "azure-openai-responses");
    assert_eq!(provider.model_id(), "gpt-test");
    assert_eq!(
        provider
            .complete(request(vec![Message::user("hi")]))
            .await
            .unwrap()
            .content,
        "ok"
    );
    let requests = server.await.unwrap();
    assert_eq!(requests[0].body["model"], "my-deployment");
    assert_eq!(requests[0].headers["api-key"], "azure-key");
    assert!(!requests[0].headers.contains_key("authorization"));
}

#[tokio::test]
async fn native_http_rate_limit_preserves_retry_after() {
    let reply = Reply {
        status: 429,
        mime: "application/json",
        body: b"{\"error\":\"limited\"}".to_vec(),
        extra: "Retry-After: 7\r\n",
    };
    let (base, server) = mock(vec![reply]).await;
    let provider = native(config("google", &base)).unwrap();
    let error = provider
        .complete(request(vec![Message::user("hi")]))
        .await
        .unwrap_err();
    assert_eq!(error.error_class(), ErrorClass::Retryable);
    assert_eq!(error.retry_after(), Some(Duration::from_secs(7)));
    server.await.unwrap();
}

#[tokio::test]
async fn embedded_errors_and_truncated_streams_never_become_success() {
    for (id, reply, retryable) in [
        (
            "anthropic",
            Reply::sse(vec![
                json!({"type":"error","error":{"type":"overloaded_error","message":"busy"}}),
            ]),
            true,
        ),
        (
            "google",
            Reply::sse(vec![
                json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}),
            ]),
            false,
        ),
        (
            "radius",
            Reply::sse(vec![
                json!({"type":"error","reason":"error","errorMessage":"failed"}),
            ]),
            false,
        ),
        (
            "amazon-bedrock",
            Reply {
                status: 200,
                mime: "application/vnd.amazon.eventstream",
                body: vec![0, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                extra: "",
            },
            false,
        ),
    ] {
        let (base, server) = mock(vec![reply]).await;
        let provider = native(config(id, &base)).unwrap();
        let error = provider
            .complete(request(vec![Message::user("hi")]))
            .await
            .unwrap_err();
        assert_eq!(
            error.error_class() == ErrorClass::Retryable,
            retryable,
            "{id}: {error}"
        );
        server.await.unwrap();
    }
}

#[test]
fn all_requested_providers_have_real_adapters_independent_of_resource_configuration() {
    for id in [
        "cloudflare-ai-gateway",
        "cloudflare-workers-ai",
        "google",
        "anthropic",
        "azure-openai-responses",
        "radius",
        "amazon-bedrock",
        "google-vertex",
    ] {
        assert!(model::provider_supported(id), "{id}");
        assert_eq!(model::provider_unsupported_reason(id), None);
    }
    assert!(!model::provider_supported("github-copilot"));
    for base in [
        "https://demo.openai.azure.com",
        "https://demo.cognitiveservices.azure.com/openai",
        "https://demo.ai.azure.com/openai/v1",
        "https://demo.openai.azure.com/openai/v1/responses",
    ] {
        assert!(
            model::azure_endpoint(base)
                .unwrap()
                .contains("/openai/v1/responses")
        );
    }
    assert!(matches!(
        native(config("unknown", "http://localhost")),
        Err(ModelError::Configuration(_))
    ));
}

#[tokio::test]
async fn imported_gemini_history_does_not_replay_foreign_signatures() {
    let (base, server) = mock(vec![Reply::sse(vec![
        json!({"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}]}),
    ])])
    .await;
    let mut config = config("google", &base);
    config.model = "gemini-3-flash".into();
    let provider = native(config).unwrap();
    let mut imported = Message::assistant(
        "old",
        vec![model::ToolCall {
            id: "old-call".into(),
            kind: "function".into(),
            function: model::FunctionCall {
                name: "read_file".into(),
                arguments: "{\"path\":\"a\"}".into(),
            },
        }],
    );
    imported.provider_metadata = Some(
        json!({"provider":"google","model":"different-model","content":[{"functionCall":{"id":"old-call","name":"read_file","args":{"path":"a"}},"thoughtSignature":"foreign-signature"}]}),
    );
    provider
        .complete(request(vec![
            Message::user("read"),
            imported,
            Message::tool("old-call", "code"),
        ]))
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(
        requests[0].body["contents"][1]["parts"][1]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    assert!(!requests[0].body.to_string().contains("foreign-signature"));
}

#[tokio::test]
async fn workers_ai_existing_bearer_request_format_is_retained() {
    let (base, server) = mock(vec![Reply::json(
        &json!({"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}),
    )])
    .await;
    let mut config = config("cloudflare-workers-ai", &base);
    config.base_url = Some(format!("{base}/accounts/account/ai/v1/chat/completions"));
    let provider = provider_adapter_with_client(config, client()).unwrap();
    provider
        .complete(request(vec![Message::user("hi")]))
        .await
        .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests[0].headers["authorization"], "Bearer test-key");
    assert!(!requests[0].headers.contains_key("cf-aig-authorization"));
    assert_eq!(requests[0].path, "/accounts/account/ai/v1/chat/completions");
}
