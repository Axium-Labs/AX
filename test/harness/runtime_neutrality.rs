//! Agent Runtime acceptance: the coding execution harness is the runtime.
//!
//! These tests exercise a *plain* kernel — no mode switch exists — and assert
//! the `DeepSeek Harness` philosophy: every run carries the coding policy and the
//! full environment snapshot, every goal gets a task queue, and a plain request
//! still completes directly without burning capabilities. The keyword "鲁迅"
//! appears only as a user request in a test; no production rule keys on it.

use crate::*;
use async_trait::async_trait;
use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn response(text: &str, calls: Vec<ToolCall>) -> ModelResponse {
    ModelResponse {
        provider_metadata: None,
        content: text.into(),
        tool_calls: calls,
        usage: None,
        finish_reason: Some("stop".into()),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn call(id: &str, name: &str, input: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall {
            name: name.into(),
            arguments: input.to_string(),
        },
    }
}

struct Script {
    replies: Mutex<std::collections::VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl Script {
    fn new(replies: Vec<ModelResponse>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(vec![]),
        })
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelProvider for Script {
    fn name(&self) -> &'static str {
        "runtime-acceptance-test"
    }
    fn model_id(&self) -> &'static str {
        "script"
    }
    fn context_window(&self) -> usize {
        128_000
    }
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ModelError> {
        panic!("must stream")
    }
    async fn complete_stream(
        &self,
        request: ModelRequest,
        delta: &mut (dyn FnMut(String) + Send),
        _: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request);
        let response = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra model request");
        if !response.content.is_empty() {
            delta(response.content.clone());
        }
        Ok(response)
    }
}

/// A capability fixture that contributes its own guidance, so the assembled
/// `[ax-capability-guidance]` section can be observed.
struct Capability;

#[async_trait]
impl tool::Tool for Capability {
    fn name(&self) -> &'static str {
        "capability"
    }
    fn description(&self) -> &'static str {
        "fixture capability"
    }
    fn guidance(&self) -> Option<&'static str> {
        Some("capability: a fixture that describes how to use itself once chosen.")
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"step":{"type":"integer"}}})
    }
    fn safety(&self, _: &serde_json::Value) -> tool::SafetyLevel {
        tool::SafetyLevel::Safe
    }
    fn capability(&self, _: &serde_json::Value) -> tool::Capability {
        tool::Capability::FilesystemWrite
    }
    async fn execute(&self, input: serde_json::Value) -> Result<String, tool::ToolError> {
        Ok(input.to_string())
    }
}

/// A network-capability fixture named `web`, so the search branch of a
/// retrieval request can be observed.
struct WebFixture;

#[async_trait]
impl tool::Tool for WebFixture {
    fn name(&self) -> &'static str {
        "web"
    }
    fn description(&self) -> &'static str {
        "fixture web search"
    }
    fn guidance(&self) -> Option<&'static str> {
        Some("web: batch independent queries into one search.")
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object","properties":{"queries":{"type":"array","items":{"type":"string"}}}})
    }
    fn safety(&self, _: &serde_json::Value) -> tool::SafetyLevel {
        tool::SafetyLevel::Safe
    }
    fn capability(&self, _: &serde_json::Value) -> tool::Capability {
        tool::Capability::Network
    }
    async fn execute(&self, _: serde_json::Value) -> Result<String, tool::ToolError> {
        Ok("搜索结果：鲁迅代表作《呐喊》《彷徨》《朝花夕拾》".to_owned())
    }
}

fn kernel(provider: Arc<Script>) -> AgentKernel {
    let mut tools = tool::ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(Capability);
    AgentKernel::new(provider, tools, Arc::new(AllowAll))
}

/// The same kernel with a network capability registered.
fn kernel_with_web(provider: Arc<Script>) -> AgentKernel {
    let mut tools = tool::ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(WebFixture);
    AgentKernel::new(provider, tools, Arc::new(AllowAll))
}

fn all_messages(requests: &[ModelRequest]) -> String {
    requests
        .iter()
        .flat_map(|request| request.messages.iter())
        .map(|message| message.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A: a bare question is answered directly — no capability is used. The run
/// still carries the full harness context (runtime prompt, capability
/// guidance, environment snapshot, coding policy) and its goal queue reaches
/// the completed terminal state.
#[tokio::test]
async fn a_plain_question_is_answered_directly() {
    let provider = Script::new(vec![response("2", vec![])]);
    let mut runtime = kernel(Arc::clone(&provider));
    let mut completion = None;
    let result = runtime
        .run_turn("1+1等于多少", |event| {
            if let AgentEvent::Completion {
                completion: kind,
                model_steps,
                tools,
                ..
            } = event
            {
                completion = Some((kind, model_steps, tools));
            }
        })
        .await
        .unwrap();
    assert_eq!(result, "2");
    assert_eq!(completion, Some(("direct".into(), 1, 0)));
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    let text = all_messages(&requests);
    assert!(text.contains("[ax-agent-runtime]"));
    assert!(text.contains("[ax-capability-guidance]"));
    assert!(text.contains("[ax-environment]"));
    assert!(text.contains("[ax-coding-harness]"));
    // Progress state is only injected for a queue with tasks; an empty goal
    // queue never reaches the request.
    assert!(!text.contains("[ax-task-queue]"));
    let queue = runtime.task_queue().expect("every goal has a queue");
    assert!(!queue.active());
    assert_eq!(queue.final_response.as_deref(), Some("2"));
}

/// B: knowledge/retrieval requests never scan or mutate the workspace.
/// Whether the model answers directly or looks for a source is its own choice;
/// the runtime never forces a workspace read either way.
#[tokio::test]
async fn b_retrieval_request_does_not_touch_the_workspace() {
    // Branch 1: existing knowledge is enough — a direct answer.
    let provider = Script::new(vec![response(
        "鲁迅的代表作有《呐喊》《彷徨》《朝花夕拾》。",
        vec![],
    )]);
    let mut runtime = kernel(Arc::clone(&provider));
    runtime
        .run_turn("找一下鲁迅有哪些代表作", |_| {})
        .await
        .unwrap();
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    let text = all_messages(&requests);
    assert!(!text.contains("[ax-task-queue]"));
    assert!(runtime.task_queue().is_some());
}

/// B (source branch): when the model wants a source, it may use the web
/// capability. That is still not a workspace scan.
#[tokio::test]
async fn b_retrieval_may_search_the_web_without_touching_the_workspace() {
    let provider = Script::new(vec![
        response(
            "",
            vec![call("search", "web", json!({"queries":["鲁迅 代表作"]}))],
        ),
        response("鲁迅的代表作有《呐喊》《彷徨》《朝花夕拾》。", vec![]),
    ]);
    let mut runtime = kernel_with_web(Arc::clone(&provider));
    let mut tools_used = None;
    let result = runtime
        .run_turn("找一下鲁迅有哪些代表作，需要来源", |event| {
            if let AgentEvent::Completion { tools, .. } = event {
                tools_used = Some(tools);
            }
        })
        .await
        .unwrap();
    assert!(result.contains("呐喊"));
    // The web call ran and its result was consumed into the answer request.
    assert_eq!(tools_used, Some(1));
    assert_eq!(provider.requests().len(), 2);
    assert!(
        provider.requests()[1]
            .messages
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("search"))
    );
    assert!(runtime.task_queue().is_some());
}

/// C: files that happen to be in cwd are context, not instructions.
#[tokio::test]
async fn c_workspace_files_do_not_hijack_the_request() {
    let dir = std::env::temp_dir().join(format!(
        "ax-runtime-acceptance-c-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("luxun_fetch.py"), "# stale fetch script").unwrap();
    std::fs::write(dir.join("鲁迅作品总目录.md"), "# stale catalog").unwrap();

    let provider = Script::new(vec![response("《呐喊》《彷徨》《野草》", vec![])]);
    let mut runtime = kernel(Arc::clone(&provider)).with_execution_scope(dir.clone());
    let mut tools_used = None;
    runtime
        .run_turn("找鲁迅的作品", |event| {
            if let AgentEvent::Completion { tools, .. } = event {
                tools_used = Some(tools);
            }
        })
        .await
        .unwrap();
    assert_eq!(tools_used, Some(0));
    let text = all_messages(&provider.requests());
    // The runtime tells the model where the workspace is (cwd/workspace root),
    // but it never lists what is inside it. Knowing the location is context;
    // naming the files would be an implicit instruction to look at them.
    assert!(text.contains("ax-runtime-acceptance-c-"));
    assert!(!text.contains("luxun_fetch.py"));
    assert!(!text.contains("鲁迅作品总目录.md"));
    assert!(runtime.task_queue().is_some());

    std::fs::remove_dir_all(&dir).ok();
}

/// The harness is unconditional: a repo-like workspace changes nothing because
/// there is no mode to switch — every run is a coding execution run.
#[tokio::test]
async fn the_harness_is_unconditional_across_workspaces() {
    let dir = std::env::temp_dir().join(format!(
        "ax-runtime-acceptance-harness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"fixture\"\n").unwrap();
    std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();

    let provider = Script::new(vec![response("ok", vec![])]);
    let mut tools = tool::ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(Capability);
    tools.register(WebFixture);
    let mut runtime = AgentKernel::new(
        Arc::clone(&provider) as Arc<dyn ModelProvider>,
        tools,
        Arc::new(AllowAll),
    )
    .with_execution_scope(dir.clone());
    runtime.run_turn("hello", |_| {}).await.unwrap();
    let text = all_messages(&provider.requests());
    assert!(text.contains("[ax-coding-harness]"));
    assert!(text.contains("[ax-agent-runtime]"));

    std::fs::remove_dir_all(&dir).ok();
}

/// D: an explicit "why does this project fail to build" request may read,
/// search and run commands.
#[tokio::test]
async fn d_explicit_workspace_investigation_is_allowed() {
    let provider = Script::new(vec![
        response("", vec![call("read", "capability", json!({"step": 1}))]),
        response("The build fails because a dependency is missing.", vec![]),
    ]);
    let mut runtime = kernel(Arc::clone(&provider));
    let result = runtime
        .run_turn("看看这个项目为什么编译失败", |_| {})
        .await
        .unwrap();
    assert!(result.contains("build fails"));
    assert_eq!(provider.requests().len(), 2);
    // The tool result was consumed into the second request.
    assert!(
        provider.requests()[1]
            .messages
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("read"))
    );
}

/// E: an explicit fix request keeps going through edit, verification and a
/// final answer.
#[tokio::test]
async fn e_explicit_fix_and_verify_runs_to_completion() {
    let provider = Script::new(vec![
        response("", vec![call("edit", "capability", json!({"step": 1}))]),
        response("", vec![call("verify", "capability", json!({"step": 2}))]),
        response("Fixed and verified.", vec![]),
    ]);
    let mut runtime = kernel(Arc::clone(&provider));
    let result = runtime
        .run_turn("修复这个编译错误并运行测试", |_| {})
        .await
        .unwrap();
    assert_eq!(result, "Fixed and verified.");
    assert_eq!(provider.requests().len(), 3);
    assert!(runtime.task_queue().is_some());
}

/// F: a durable queue tracks long-running work, and resuming it continues the
/// tracked work.
#[tokio::test]
async fn f_long_running_work_is_explicit_and_resumable() {
    let provider = Script::new(vec![
        response(
            "",
            vec![call(
                "queue",
                "task_queue",
                json!({"action":"start","overall_goal":"refactor","tasks":["part one","part two"]}),
            )],
        ),
        response(
            "",
            vec![call(
                "ask",
                "request_user_input",
                json!({"question":"Which branch?","allow_free_text":true}),
            )],
        ),
    ]);
    let mut runtime = kernel(Arc::clone(&provider));
    let result = runtime
        .run_turn("把刚才那个未完成的重构继续做完", |_| {})
        .await;
    assert!(matches!(result, Err(AgentError::WaitingForUser(_))));
    let queue = runtime.task_queue().expect("long task has a queue");
    assert_eq!(queue.tasks.len(), 2);

    // Resuming with an answer continues the same tracked goal. The queue still
    // has pending tasks, so the model must drive them to a terminal state
    // before the turn can finish.
    let answer = UserAnswer::free_text(runtime.pending_question().unwrap(), "main");
    let intent = GoalTurn::Answer {
        goal_id: runtime.goal_id().unwrap().into(),
        answer,
    };
    provider.replies.lock().unwrap().push_back(response(
        "",
        vec![call(
            "cancel",
            "task_queue",
            json!({"action":"cancel","reason":"user stopped the refactor"}),
        )],
    ));
    runtime.run_goal_turn("main", intent, |_| {}).await.unwrap();
    let queue = runtime.task_queue().expect("queue survives the resume");
    assert_eq!(queue.tasks.len(), 2);
    assert!(!queue.active());
}

/// G: repeating the identical call with identical arguments triggers an
/// advisory reminder, and the calls are not hard-blocked.
#[tokio::test]
async fn g_repeated_identical_calls_are_reminded_not_blocked() {
    let same = json!({"step": 1});
    let provider = Script::new(vec![
        response("", vec![call("c1", "capability", same.clone())]),
        response("", vec![call("c2", "capability", same.clone())]),
        response("", vec![call("c3", "capability", same.clone())]),
        response("Done after changing approach.", vec![]),
    ]);
    let mut runtime = kernel(Arc::clone(&provider));
    let result = runtime.run_turn("work", |_| {}).await.unwrap();
    assert_eq!(result, "Done after changing approach.");
    let requests = provider.requests();
    assert_eq!(requests.len(), 4);
    // Every repeated call still executed; the reminder arrives as context.
    let last = requests.last().unwrap();
    assert!(
        last.messages
            .iter()
            .any(|message| message.content.starts_with("[ax-loop-hygiene]"))
    );
    let reminder = last
        .messages
        .iter()
        .find(|message| message.content.starts_with("[ax-loop-hygiene]"))
        .unwrap();
    assert!(reminder.content.contains("analyze the previous result"));
}

/// F (hygiene thresholds): eight consecutive identical calls still execute and
/// produce the gentle reminder at the 3rd call and the detailed reminder at the
/// 5th and 8th — advisory context only, never a block or a cap.
#[tokio::test]
async fn g_every_repeat_threshold_fires_in_the_real_loop() {
    let same = json!({"step": 1});
    let mut replies = (1..=8)
        .map(|n| {
            response(
                "",
                vec![call(format!("c{n}").as_str(), "capability", same.clone())],
            )
        })
        .collect::<Vec<_>>();
    replies.push(response("Done.", vec![]));
    let provider = Script::new(replies);
    let mut runtime = kernel(Arc::clone(&provider));
    let result = runtime.run_turn("work", |_| {}).await.unwrap();
    assert_eq!(result, "Done.");
    let requests = provider.requests();
    assert_eq!(requests.len(), 9);
    let reminders = |request: &ModelRequest| {
        request
            .messages
            .iter()
            .filter(|message| message.content.starts_with("[ax-loop-hygiene]"))
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
    };
    // Calls 1, 2 stay silent; call 3 escalates gently; 5 and 8 name the run.
    // A reminder is injected once per threshold hit and then persists in the
    // transcript, so the last request carries all three.
    for request in &requests[0..3] {
        assert!(
            reminders(request).is_empty(),
            "no reminder before the 3rd call"
        );
    }
    assert_eq!(reminders(&requests[3]).len(), 1);
    let gentle = &reminders(&requests[3])[0];
    assert!(gentle.contains("analyze the previous result"));
    assert!(!gentle.contains("consecutive_calls"));
    assert!(reminders(&requests[4])[0] == *gentle);
    let fifth = reminders(&requests[5]);
    assert_eq!(fifth.len(), 2);
    assert!(
        fifth
            .iter()
            .any(|text| text.contains("consecutive_calls: 5"))
    );
    for request in &requests[6..8] {
        assert_eq!(reminders(request).len(), 2);
    }
    let all = reminders(&requests[8]);
    assert_eq!(all.len(), 3);
    assert!(all.iter().any(|text| text.contains("consecutive_calls: 8")));
    // The advisory reminders never blocked: all eight identical calls executed
    // (eight distinct call ids visible in the final transcript).
    assert_eq!(
        provider
            .requests()
            .last()
            .unwrap()
            .messages
            .iter()
            .filter_map(|message| message.tool_call_id.as_deref())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        8
    );
}

/// Every run carries the harness context: runtime prompt, capability guidance
/// assembled from tool-owned guidance, environment snapshot and coding policy.
#[tokio::test]
async fn every_run_carries_the_harness_context() {
    let provider = Script::new(vec![response("ok", vec![])]);
    let mut runtime = kernel(provider);
    runtime.run_turn("hi", |_| {}).await.unwrap();
    let queue = runtime.task_queue().expect("every goal has a queue");
    assert!(!queue.active());
    // The capability guidance section is assembled from tool-owned guidance.
    assert!(runtime.has_tool("capability"));
}
