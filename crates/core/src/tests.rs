//! Kernel-level tests: the turn loop, checkpoints, compression and the
//! supervisor, exercised through the crate's public façade.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use model::{
    FunctionCall, Message, ModelError, ModelProvider, ModelRequest, ModelResponse, Role, ToolCall,
};
use serde_json::{Value, json};

use crate::{
    AgentError, AgentEvent, AgentKernel, AgentSupervisor, AgentTask, AllowAll, ContextBudget,
    ContextPoolPolicy, DenyDangerous, ExecutionBudget,
    compression::summary::{StateEntry, fit_summary, parse_saved_summary, serialize_state},
    context::request_context,
    estimate_tokens,
};
use tool::{SafetyLevel, Tool, ToolError, ToolRegistry};

struct ScriptedProvider {
    model: String,
    responses: Mutex<VecDeque<ModelResponse>>,
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    fn name(&self) -> &'static str {
        "test"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn model_id(&self) -> &str {
        &self.model
    }

    fn context_window(&self) -> usize {
        6_000
    }

    fn max_output_tokens(&self) -> Option<usize> {
        Some(100)
    }

    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.responses
            .lock()
            .expect("response mutex poisoned")
            .pop_front()
            .ok_or_else(|| ModelError::InvalidResponse("script exhausted".to_owned()))
    }
}

struct EchoTool;

#[tokio::test]
async fn failed_patch_recovers_with_local_read_patch_and_minimal_check() {
    let path = std::env::temp_dir().join(format!(
        "ax-recovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    tokio::fs::write(&path, "old\nkeep\n").await.unwrap();
    let call = |id: &str, name: &str, input: Value| ModelResponse {
        usage: None,
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: input.to_string(),
            },
        }],
        finish_reason: None,
    };
    let provider = ScriptedProvider {
        model: "recovery".into(),
        responses: Mutex::new(VecDeque::from([
            call(
                "bad",
                "patch",
                serde_json::json!({"path":path,"edits":[{"start_line":1,"delete_count":1,"expected_lines":["stale"],"new_text":"new\n"}]}),
            ),
            call(
                "read",
                "filesystem",
                serde_json::json!({"operation":"read","path":path,"start_line":1,"end_line":2}),
            ),
            call(
                "fix",
                "patch",
                serde_json::json!({"path":path,"edits":[{"start_line":1,"delete_count":1,"expected_lines":["old"],"new_text":"new\n"}]}),
            ),
            call(
                "check",
                "filesystem",
                serde_json::json!({"operation":"read","path":path,"start_line":1,"end_line":1}),
            ),
            ModelResponse {
                usage: None,
                content: "done".into(),
                tool_calls: vec![],
                finish_reason: None,
            },
        ])),
    };
    let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(tool::PatchTool);
    tools.register(tool::FilesystemTool);
    let mut kernel = AgentKernel::new(Arc::new(provider), tools, Arc::new(AllowAll))
        .with_execution_scope(path.parent().unwrap().to_owned());
    let mut finished = Vec::new();
    kernel
        .run_turn("repair this file", |event| {
            if let AgentEvent::ToolFinished {
                id,
                success,
                result,
                ..
            } = event
            {
                finished.push((id, success, result));
            }
        })
        .await
        .unwrap();
    assert_eq!(
        finished
            .iter()
            .map(|(id, ok, _)| (id.as_str(), *ok))
            .collect::<Vec<_>>(),
        [
            ("bad", false),
            ("read", true),
            ("fix", true),
            ("check", true)
        ]
    );
    assert!(
        serde_json::to_string(&finished[0].2.diagnostics)
            .unwrap()
            .contains("patch_conflict")
    );
    assert_eq!(
        tokio::fs::read_to_string(&path).await.unwrap(),
        "new\nkeep\n"
    );
    tokio::fs::remove_file(path).await.unwrap();
}

#[tokio::test]
async fn checkpoint_failure_stops_before_tool_execution_and_keeps_recoverable_history() {
    let provider = ScriptedProvider {
        model: "scripted".into(),
        responses: Mutex::new(VecDeque::from([ModelResponse {
            usage: None,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: "call".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: "echo".into(),
                    arguments: "{}".into(),
                },
            }],
            finish_reason: None,
        }])),
    };
    let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
    registry.register(EchoTool);
    let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));
    let mut saved = Vec::new();
    let mut failed_once = false;
    let result = kernel
        .run_turn_checkpointed(
            "test",
            |_| {},
            |messages| {
                if messages.iter().any(|m| !m.tool_calls.is_empty()) && !failed_once {
                    failed_once = true;
                    return Err(AgentError::Persistence("disk failure".into()));
                }
                saved = messages.to_vec();
                Ok(())
            },
        )
        .await;
    assert!(matches!(result, Err(AgentError::Persistence(_))));
    let conversation: Vec<_> = saved.iter().filter(|m| m.role != Role::System).collect();
    assert_eq!(conversation.len(), 3);
    assert_eq!(conversation[0].role, Role::User);
    assert!(conversation[2].content.contains("Execution interrupted"));
}

#[tokio::test]
async fn each_model_and_tool_message_is_checkpointed_in_order() {
    let provider = ScriptedProvider {
        model: "scripted".into(),
        responses: Mutex::new(VecDeque::from([
            ModelResponse {
                usage: None,
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "echo".into(),
                        arguments: "{}".into(),
                    },
                }],
                finish_reason: None,
            },
            ModelResponse {
                usage: None,
                content: "done".into(),
                tool_calls: vec![],
                finish_reason: None,
            },
        ])),
    };
    let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
    registry.register(EchoTool);
    let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));
    let mut lengths = Vec::new();
    kernel
        .run_turn_checkpointed(
            "test",
            |_| {},
            |messages| {
                lengths.push(messages.len());
                Ok(())
            },
        )
        .await
        .unwrap();
    // Goal admission checkpoints the empty history before the first input.
    assert_eq!(lengths, vec![0, 2, 3, 5, 6, 6]);
}

struct EchoProvider;

#[async_trait]
impl ModelProvider for EchoProvider {
    fn name(&self) -> &'static str {
        "echo"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn model_id(&self) -> &'static str {
        "echo-model"
    }

    fn context_window(&self) -> usize {
        6_000
    }

    fn max_output_tokens(&self) -> Option<usize> {
        Some(100)
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse {
            usage: None,
            content: request
                .messages
                .last()
                .map_or_else(String::new, |message| message.content.clone()),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_owned()),
        })
    }
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo input"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn capability(&self, _input: &Value) -> tool::Capability {
        tool::Capability::Process
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }

    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        Ok(input.to_string())
    }
}

#[tokio::test]
async fn loops_through_tool_result_to_final_answer() {
    let provider = ScriptedProvider {
        model: "scripted".to_owned(),
        responses: Mutex::new(VecDeque::from([
            ModelResponse {
                usage: None,
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call-1".to_owned(),
                    kind: "function".to_owned(),
                    function: FunctionCall {
                        name: "echo".to_owned(),
                        arguments: r#"{"value":"hello"}"#.to_owned(),
                    },
                }],
                finish_reason: Some("tool_calls".to_owned()),
            },
            ModelResponse {
                usage: None,
                content: "done".to_owned(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            },
        ])),
    };
    let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
    registry.register(EchoTool);
    let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));

    let answer = kernel
        .run_turn("use echo", |_| {})
        .await
        .expect("agent turn should succeed");

    assert_eq!(answer, "done");
    assert_eq!(
        kernel
            .messages()
            .iter()
            .filter(|m| m.role != Role::System)
            .count(),
        4
    );
    assert_eq!(
        kernel
            .messages()
            .iter()
            .filter(|m| m.role != Role::System)
            .nth(2)
            .unwrap()
            .role,
        Role::Tool
    );
}

#[tokio::test]
async fn compresses_old_context_without_truncating_recent_messages() {
    // Trigger comes from ContextBudget pressure alone; no configured
    // percentage participates in the decision.
    let provider = ScriptedProvider {
        model: "scripted".to_owned(),
        responses: Mutex::new(VecDeque::from([ModelResponse {
            usage: None,
            content: r#"{"state":[{"type":"goal","content":"goal and decisions preserved","importance":0.9}]}"#.to_owned(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_owned()),
        }])),
    };
    let messages = (0..6)
        .map(|index| Message::user(format!("{index}:{}", "x".repeat(3500))))
        .collect();
    let mut kernel = AgentKernel::new(
        Arc::new(provider),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(DenyDangerous),
    )
    .with_messages(messages);

    kernel.configure_context_pool(ContextPoolPolicy {
        recent_raw_maximum: 1000,
        ..Default::default()
    });
    let result = kernel
        .compress_if_needed(|_| {})
        .await
        .expect("compression should succeed")
        .expect("context should exceed threshold");

    assert_eq!(result.removed_messages, 5);
    assert_eq!(kernel.messages().len(), 2);
    assert!(kernel.messages()[0].content.contains("goal and decisions"));
    assert!(kernel.messages()[1].content.starts_with("5:"));
}

#[test]
fn request_context_fits_history_memory_skills_and_large_tool_schema() {
    let budget = ContextBudget::new(6_000, Some(500), 1_200);
    let mut messages = (0..30)
        .map(|index| Message::user(format!("old {index}:{}", "x".repeat(400))))
        .collect::<Vec<_>>();
    messages.push(Message::system(format!(
        "[retrieved-memory]\n{}",
        "m".repeat(500)
    )));
    messages.push(Message::system(format!(
        "[ax-skill:first]\n{}",
        "s".repeat(2_000)
    )));
    messages.push(Message::system(format!(
        "[ax-skill:second]\n{}",
        "s".repeat(2_000)
    )));
    messages.push(Message::user("current request"));

    let selected = request_context(&messages, budget).expect("request should fit");
    assert!(estimate_tokens(&selected) <= budget.usable());
    assert!(
        selected
            .iter()
            .any(|message| message.content == "current request")
    );
    assert!(
        selected
            .iter()
            .filter(|message| message.content.starts_with("old "))
            .count()
            < 30
    );
    assert_eq!(
        messages.len(),
        34,
        "selection must not mutate source history"
    );
}

#[test]
fn tiny_context_rejects_an_oversized_latest_turn() {
    let budget = ContextBudget::new(1_000, Some(100), 700);
    let messages = vec![Message::user("x".repeat(2_000))];
    assert!(matches!(
        request_context(&messages, budget),
        Err(AgentError::Budget(_))
    ));
}

#[tokio::test]
async fn compacted_context_stays_within_final_request_budget() {
    let provider = ScriptedProvider {
        model: "scripted".to_owned(),
        responses: Mutex::new(VecDeque::from([ModelResponse {
            usage: None,
            content:
                r#"{"state":[{"type":"progress","content":"short summary","importance":0.8}]}"#
                    .to_owned(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_owned()),
        }])),
    };
    let messages = (0..30)
        .map(|index| Message::user(format!("{index}:{}", "x".repeat(600))))
        .collect();
    let mut kernel = AgentKernel::new(
        Arc::new(provider),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(DenyDangerous),
    )
    .with_messages(messages);
    assert!(kernel.compress_if_needed(|_| {}).await.unwrap().is_some());
    kernel.push_context(Message::system(format!(
        "[retrieved-memory]\n{}",
        "m".repeat(500)
    )));
    kernel.push_context(Message::system(format!(
        "[ax-skill:test]\n{}",
        "s".repeat(2_000)
    )));
    kernel.push_context(Message::user("new question"));
    let selected = request_context(kernel.messages(), kernel.context_budget()).unwrap();
    assert!(estimate_tokens(&selected) <= kernel.context_budget().usable());
    assert!(
        selected
            .iter()
            .any(|message| message.content == "new question")
    );
}

struct RecordingProvider {
    replies: Mutex<VecDeque<ModelResponse>>,
    request_tokens: Mutex<Vec<usize>>,
}

#[async_trait]
impl ModelProvider for RecordingProvider {
    fn name(&self) -> &'static str {
        "recording"
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn model_id(&self) -> &'static str {
        "recording"
    }
    fn context_window(&self) -> usize {
        6_000
    }
    fn max_output_tokens(&self) -> Option<usize> {
        Some(100)
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        if request
            .messages
            .first()
            .is_some_and(|m| m.content.starts_with("Compress the older conversation"))
        {
            return Ok(ModelResponse {
                usage: None,
                content: r#"{"state":[{"type":"goal","content":"finish","importance":0.9}]}"#
                    .into(),
                tool_calls: vec![],
                finish_reason: None,
            });
        }
        self.request_tokens
            .lock()
            .unwrap()
            .push(estimate_tokens(&request.messages));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ModelError::InvalidResponse("script exhausted".into()))
    }
}

struct LargeTestTool;
#[async_trait]
impl Tool for LargeTestTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &'static str {
        "test"
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn description(&self) -> &str {
        "Run tests"
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object"})
    }
    fn capability(&self, _: &Value) -> tool::Capability {
        tool::Capability::Process
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        if input["large"] == true {
            Ok(format!(
                "cargo test\n{}\ntest result: FAILED. 243 passed; 1 failed\nparser::tests::nested\nsrc/parser.rs:281 stack overflow\nexit=101",
                "progress line\n".repeat(2000)
            ))
        } else {
            Ok("quick check passed".into())
        }
    }
}

fn test_call(id: &str, large: bool) -> ToolCall {
    ToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall {
            name: "test".into(),
            arguments: format!("{{\"large\":{large}}}"),
        },
    }
}

#[tokio::test]
async fn compresses_between_tool_calls_and_preserves_raw_turn() {
    let provider = Arc::new(RecordingProvider {
        replies: Mutex::new(VecDeque::from([
            ModelResponse {
                usage: None,
                content: String::new(),
                tool_calls: vec![test_call("a", true)],
                finish_reason: None,
            },
            ModelResponse {
                usage: None,
                content: String::new(),
                tool_calls: vec![test_call("b", false)],
                finish_reason: None,
            },
            ModelResponse {
                usage: None,
                content: "done".into(),
                tool_calls: vec![],
                finish_reason: None,
            },
        ])),
        request_tokens: Mutex::new(vec![]),
    });
    let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
    registry.register(LargeTestTool);
    let mut kernel = AgentKernel::new(provider.clone(), registry, Arc::new(DenyDangerous));
    let mut events = Vec::new();
    assert_eq!(
        kernel
            .run_turn("run tests", |e| events.push(e))
            .await
            .unwrap(),
        "done"
    );
    let sizes = provider.request_tokens.lock().unwrap().clone();
    assert_eq!(sizes.len(), 3);
    assert!(
        sizes[1] < 1000,
        "second request should see reduced test output: {sizes:?}"
    );
    assert!(
        kernel
            .messages()
            .iter()
            .any(|message| message.role == Role::Tool && message.content.len() < 4000)
    );
    assert!(
        kernel
            .messages()
            .iter()
            .any(|m| m.content.contains("243 passed; 1 failed"))
    );
    let raw = kernel.take_turn_messages();
    assert!(raw.iter().any(|m| m.content.len() > 20_000));
}

#[tokio::test]
async fn semantic_state_keeps_early_constraints_and_failures_without_summary_recursion() {
    let provider = ScriptedProvider { model: "scripted".into(), responses: Mutex::new(VecDeque::from([
        ModelResponse { usage: None, content: r#"{"state":[{"type":"constraint","content":"Do not change the public API","importance":1.0},{"type":"failure","content":"Approach A failed because of a parser stack overflow","importance":0.95}]}"#.into(), tool_calls: vec![], finish_reason: None }
    ])) };
    let mut kernel = AgentKernel::new(
        Arc::new(provider),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(DenyDangerous),
    )
    .with_messages(vec![
        Message::system("[memory-summary]\nDECISIONS: keep SQLite"),
        Message::user(format!(
            "Do not change the public API\n{}",
            "task details ".repeat(1500)
        )),
        Message::assistant(
            "Approach A failed because of a parser stack overflow",
            vec![],
        ),
        Message::user("continue"),
    ]);
    let result = kernel.compact_now(|_| {}).await.unwrap().unwrap();
    assert!(result.semantic_called);
    assert!(result.summary.contains("Do not change the public API"));
    assert!(
        result
            .summary
            .contains("Approach A failed because of a parser stack overflow")
    );
    assert!(result.summary.contains("DECISIONS: keep SQLite"));
    assert_eq!(kernel.messages().last().unwrap().content, "continue");
}

#[tokio::test]
async fn repeated_file_reads_collapse_older_tool_result() {
    let provider = Arc::new(RecordingProvider {
        replies: Mutex::new(VecDeque::new()),
        request_tokens: Mutex::new(vec![]),
    });
    let read = |id: &str| ToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall {
            name: "read_file".into(),
            arguments: "{\"path\":\"src/lib.rs\"}".into(),
        },
    };
    let mut kernel = AgentKernel::new(
        provider,
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(DenyDangerous),
    )
    .with_messages(vec![
        Message::user("inspect file"),
        Message::assistant("", vec![read("first")]),
        Message::tool("first", "old file body \n".repeat(500)),
        Message::assistant("", vec![read("second")]),
        Message::tool("second", "new file body \n".repeat(500)),
        Message::user("current task"),
    ]);
    kernel.configure_context_pool(ContextPoolPolicy {
        recent_raw_maximum: 100,
        ..Default::default()
    });
    kernel.compact_now(|_| {}).await.unwrap();
    assert!(
        kernel
            .messages()
            .iter()
            .any(|m| m.content.contains("duplicate tool output"))
    );
}

#[test]
fn structured_summary_uses_importance_over_keywords_and_type() {
    let entries = vec![
        StateEntry {
            kind: "other".into(),
            content: "All deliverables use British English".into(),
            importance: 0.95,
        },
        StateEntry {
            kind: "error".into(),
            content: "The word error appeared in a harmless example".into(),
            importance: 0.05,
        },
    ];
    let high_only = serialize_state(&entries[..1]);
    let budget = estimate_tokens(&[Message::system(high_only)]) + 2;
    let fitted = fit_summary(&entries, budget).unwrap();
    let parsed = parse_saved_summary(&fitted);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].content, "All deliverables use British English");
    assert!(estimate_tokens(&[Message::system(fitted)]) <= budget);
}

#[tokio::test]
async fn semantic_model_can_preserve_constraint_without_keyword() {
    let provider = ScriptedProvider { model: "scripted".into(), responses: Mutex::new(VecDeque::from([
        ModelResponse { usage: None, content: r#"{"state":[{"type":"constraint","content":"All deliverables use British English","importance":0.98}]}"#.into(), tool_calls: vec![], finish_reason: None }
    ])) };
    let mut kernel = AgentKernel::new(
        Arc::new(provider),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(DenyDangerous),
    )
    .with_messages(vec![
        Message::user(format!(
            "All deliverables use British English\n{}",
            "background ".repeat(2000)
        )),
        Message::assistant("noted", vec![]),
        Message::user("continue"),
    ]);
    let result = kernel.compact_now(|_| {}).await.unwrap().unwrap();
    assert!(
        result
            .summary
            .contains("All deliverables use British English")
    );
    let selected = request_context(kernel.messages(), kernel.context_budget()).unwrap();
    assert!(estimate_tokens(&selected) <= kernel.context_budget().usable());
}

#[tokio::test]
async fn malformed_structured_output_keeps_original_context() {
    let provider = ScriptedProvider {
        model: "scripted".into(),
        responses: Mutex::new(VecDeque::from([ModelResponse {
            usage: None,
            content: "{broken JSON".into(),
            tool_calls: vec![],
            finish_reason: None,
        }])),
    };
    let mut kernel = AgentKernel::new(
        Arc::new(provider),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(DenyDangerous),
    )
    .with_messages(vec![
        Message::user("long context ".repeat(2000)),
        Message::assistant("old answer", vec![]),
        Message::user("continue"),
    ]);
    let original = serde_json::to_string(kernel.messages()).unwrap();
    assert!(kernel.compact_now(|_| {}).await.unwrap().is_none());
    assert_eq!(serde_json::to_string(kernel.messages()).unwrap(), original);
    assert!(!kernel.take_compression_dirty());
}

#[tokio::test]
async fn supervisor_runs_independent_tasks() {
    let template = AgentKernel::new(
        Arc::new(EchoProvider),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(DenyDangerous),
    );
    let supervisor = AgentSupervisor::new(template, 2);
    let results = supervisor
        .run_tasks(
            vec![
                AgentTask {
                    id: "b".to_owned(),
                    prompt: "second".to_owned(),
                    context: Vec::new(),
                },
                AgentTask {
                    id: "a".to_owned(),
                    prompt: "first".to_owned(),
                    context: Vec::new(),
                },
            ],
            None,
        )
        .await
        .expect("supervisor should finish");

    assert_eq!(results[0].id, "a");
    assert_eq!(
        results[0]
            .result
            .as_deref()
            .expect("first agent should succeed"),
        "first"
    );
    assert_eq!(
        results[1]
            .result
            .as_deref()
            .expect("second agent should succeed"),
        "second"
    );
}

#[tokio::test]
async fn tool_budget_stops_before_execution_and_keeps_valid_transcript() {
    let provider = ScriptedProvider {
        model: "test".into(),
        responses: Mutex::new(VecDeque::from([ModelResponse {
            usage: None,
            content: String::new(),
            tool_calls: vec![
                ToolCall {
                    id: "pending-1".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "echo".into(),
                        arguments: "{}".into(),
                    },
                },
                ToolCall {
                    id: "pending-2".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "echo".into(),
                        arguments: "{}".into(),
                    },
                },
            ],
            finish_reason: None,
        }])),
    };
    let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(EchoTool);
    let mut kernel = AgentKernel::new(Arc::new(provider), tools, Arc::new(AllowAll))
        .with_execution_budget(ExecutionBudget {
            max_tool_calls: 1,
            ..ExecutionBudget::default()
        });
    assert!(matches!(
        kernel.run_turn("test", |_| {}).await,
        Err(AgentError::Budget(_))
    ));
    assert_eq!(
        kernel.messages().last().unwrap().tool_call_id.as_deref(),
        Some("pending-2")
    );
    assert!(
        kernel
            .messages()
            .last()
            .unwrap()
            .content
            .contains("interrupted")
    );
}

struct PendingProvider;
#[async_trait]
impl ModelProvider for PendingProvider {
    fn name(&self) -> &'static str {
        "pending"
    }
    fn model_id(&self) -> &'static str {
        "pending"
    }
    /// Large enough that the request is not rejected for context pressure: the
    /// stall, not a budget error, is what the assertion is about.
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn turn_timeout_is_enforced_while_model_is_waiting() {
    let mut kernel = AgentKernel::new(
        Arc::new(PendingProvider),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(AllowAll),
    )
    .with_execution_budget(ExecutionBudget {
        turn_timeout_secs: 1,
        ..ExecutionBudget::default()
    });
    assert!(matches!(
        kernel.run_turn("wait", |_| {}).await,
        Err(AgentError::Timeout(_))
    ));
    assert_eq!(
        kernel
            .messages()
            .iter()
            .find(|m| m.role == Role::User)
            .unwrap()
            .content,
        "wait"
    );
}
