use super::*;
use async_trait::async_trait;
use model::{FunctionCall, ModelResponse, Role, ToolCall};
use serde_json::json;
use std::{path::PathBuf, sync::Mutex};

fn state(root: &std::path::Path) -> ExecutionState {
    let mut state = ExecutionState::default();
    state.begin("produce the requested dataset", "goal-test", root);
    state
}
fn binding(step: &str, scope: &std::path::Path) -> Value {
    json!({"goal_id":"goal-test","step":step,"expected_output":"loaded dataset","scope":[scope]})
}
fn temp() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "ax-execution-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}
#[test]
fn recovery_stays_at_failed_step_and_returns_after_corrected_retry() {
    let root = temp();
    let task = root.join("dataset");
    std::fs::create_dir_all(&task).unwrap();
    let mut state = state(&root);
    let input = json!({"operation":"read","path":task.join("data.parquet"),"_ax_execution":binding("load parquet", &task)});
    let input = state.prepare(&tool::FilesystemTool, input).unwrap();
    state.record(
        "failed",
        &tool::FilesystemTool,
        &input,
        &tool::ToolResult::new(false, "missing input".into()),
    );
    assert_eq!(state.recovery_for.as_deref(), Some("load parquet"));
    let outside = json!({"operation":"list","path":root.join("other-project")});
    assert!(state.prepare(&tool::FilesystemTool, outside).is_err());
    let drift = json!({"operation":"list","path":task,"_ax_execution":binding("inspect unrelated project", &task)});
    assert!(state.prepare(&tool::FilesystemTool, drift).is_err());
    assert_eq!(state.current_step, "load parquet");
    let mut retry = input.clone();
    retry["start_line"] = json!(1);
    state.prepare(&tool::FilesystemTool, retry.clone()).unwrap();
    state.record(
        "retry",
        &tool::FilesystemTool,
        &retry,
        &tool::ToolResult::new(true, "dataset".into()),
    );
    assert!(state.recovery_for.is_none());
    assert_eq!(state.current_step, "load parquet");
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn three_failed_retries_require_different_bounded_strategy() {
    let root = temp();
    let mut state = state(&root);
    let input = json!({"operation":"read","path":root.join("missing")});
    for n in 0..3 {
        state.record(
            &n.to_string(),
            &tool::FilesystemTool,
            &input,
            &tool::ToolResult::new(false, "missing".into()),
        );
    }
    assert!(state.prepare(&tool::FilesystemTool, input).is_err());
    assert!(state.progress.no_progress);
    assert_eq!(state.progress.strategy_changes, 1);
    assert!(
        state
            .prepare(&tool::ShellTool, json!({"command":"scan entire workspace"}))
            .is_err()
    );
    let repair = json!({"operation":"write","path":root.join("missing"),"content":"data"});
    state
        .prepare(&tool::FilesystemTool, repair.clone())
        .unwrap();
    state.record(
        "repair",
        &tool::FilesystemTool,
        &repair,
        &tool::ToolResult::new(true, "written".into()),
    );
    assert_eq!(state.progress.advances, 1);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn success_of_reads_and_changing_steps_do_not_hide_no_progress() {
    let root = temp();
    let mut state = state(&root);
    for n in 0..8 {
        let input = json!({"operation":"read","path":root.join(format!("file-{n}")),"_ax_execution":binding("inspect input", &root)});
        let input = state.prepare(&tool::FilesystemTool, input).unwrap();
        state.record(
            &n.to_string(),
            &tool::FilesystemTool,
            &input,
            &tool::ToolResult::new(true, "observed".into()),
        );
    }
    assert!(state.progress.no_progress);
    assert_eq!(state.progress.advances, 0);
    assert!(state.prepare(&tool::FilesystemTool, json!({"operation":"list","path":root,"_ax_execution":binding("new step without progress", &root)})).is_err());
    assert!(state.context(8).content.contains("No progress"));
    assert!(state.context(8).content.contains("file-7"));
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn goal_identity_scope_and_duplicate_evidence_are_enforced() {
    let root = temp();
    let mut state = state(&root);
    let mut wrong = binding("load parquet", &root);
    wrong["goal_id"] = json!("other-goal");
    assert!(
        state
            .prepare(
                &tool::FilesystemTool,
                json!({"operation":"read","path":root,"_ax_execution":wrong})
            )
            .is_err()
    );
    let input = json!({"operation":"read","path":root,"_ax_observe":"/loaded"});
    let result = tool::ToolResult::new(true, "{\"loaded\":true}".into());
    state.record("a", &tool::FilesystemTool, &input, &result);
    state.record("b", &tool::FilesystemTool, &input, &result);
    assert_eq!(state.progress.advances, 1);
    std::fs::remove_dir_all(root).unwrap();
}

struct Recorder {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}
#[async_trait]
impl ModelProvider for Recorder {
    fn name(&self) -> &'static str {
        "execution-test"
    }
    fn model_id(&self) -> &'static str {
        "execution-test"
    }
    fn context_window(&self) -> usize {
        32_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request);
        Ok(self.responses.lock().unwrap().pop_front().unwrap())
    }
}
fn text(content: &str) -> ModelResponse {
    ModelResponse {
        content: content.into(),
        tool_calls: vec![],
        usage: None,
        finish_reason: None,
    }
}
#[allow(clippy::needless_pass_by_value)]
fn call(n: usize, input: Value) -> ModelResponse {
    ModelResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: format!("call-{n}"),
            kind: "function".into(),
            function: FunctionCall {
                name: "filesystem".into(),
                arguments: input.to_string(),
            },
        }],
        usage: None,
        finish_reason: None,
    }
}
fn recorder(responses: Vec<ModelResponse>) -> Arc<Recorder> {
    Arc::new(Recorder {
        responses: Mutex::new(responses.into()),
        requests: Mutex::new(vec![]),
    })
}
fn kernel(provider: Arc<Recorder>, root: &std::path::Path) -> AgentKernel {
    let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(tool::FilesystemTool);
    AgentKernel::new(provider, tools, Arc::new(AllowAll)).with_execution_scope(root.to_owned())
}
#[tokio::test]
async fn loop_corrects_eight_ineffective_calls_and_history_survives_compression_and_resume() {
    let root = temp();
    let input_file = root.join("input.txt");
    std::fs::write(&input_file, "dataset").unwrap();
    let output = root.join("output.txt");
    let mut responses: Vec<_> = (0..8)
        .map(|n| call(n, json!({"operation":"read","path":input_file})))
        .collect();
    responses.push(call(8, json!({"operation":"read","path":input_file})));
    responses.push(call(
        9,
        json!({"operation":"write","path":output,"content":"requested result"}),
    ));
    responses.push(text("done"));
    let provider = recorder(responses);
    let mut kernel = kernel(provider.clone(), &root);
    kernel.run_turn("produce output.txt", |_| {}).await.unwrap();
    {
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 11); // no planner or detection model calls
        let corrected = requests[8]
            .messages
            .iter()
            .find(|m| m.content.starts_with(execution::CONTEXT_PREFIX))
            .unwrap();
        assert!(corrected.content.contains("\"no_progress\":true"));
        assert!(corrected.content.contains("produce output.txt"));
    }
    let rejected = kernel
        .raw_turn_messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("call-8"))
        .unwrap();
    let result: tool::ToolResult = serde_json::from_str(&rejected.content).unwrap();
    assert_ne!(result.status, "success");
    assert!(result.raw_output.contains("no_progress"));
    assert_eq!(std::fs::read_to_string(output).unwrap(), "requested result");
    assert_eq!(kernel.execution_state().progress.advances, 1);
    assert!(!kernel.execution_state().progress.no_progress);
    // Compaction never owns execution state; force a semantic compact of old history.
    kernel
        .messages
        .insert(0, Message::user("old context ".repeat(5000)));
    kernel.messages.push(Message::user("continue"));
    provider.responses.lock().unwrap().push_back(text(
        "{\"state\":[{\"type\":\"other\",\"content\":\"old\",\"importance\":1.0}]}",
    ));
    kernel.compact_now(|_| {}).await.unwrap();
    assert_eq!(kernel.execution_state().overall_goal, "produce output.txt");
    assert_eq!(kernel.execution_state().current_step, "produce output.txt");
    let saved = kernel.take_turn_messages();
    let last = saved
        .iter()
        .rev()
        .find(|m| m.content.starts_with(execution::STATE_PREFIX))
        .unwrap()
        .clone();
    let history_provider = recorder(vec![text(
        "ten attempts were recorded, including one rejected read",
    )]);
    let mut restored = super::AgentKernel::new(
        history_provider.clone(),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(AllowAll),
    )
    .with_messages(vec![last]);
    restored.run_turn("刚才做了什么", |_| {}).await.unwrap();
    let request = &history_provider.requests.lock().unwrap()[0];
    let events = request
        .messages
        .iter()
        .find(|m| m.content.starts_with(execution::CONTEXT_PREFIX))
        .unwrap();
    let summary: Value = serde_json::from_str(
        events
            .content
            .strip_prefix(execution::CONTEXT_PREFIX)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary["total_tool_calls"], 10);
    assert_eq!(summary["actual_recent_tool_events"][0]["id"], "call-9");
    assert_eq!(
        summary["actual_recent_tool_events"][0]["tool"],
        "filesystem"
    );
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.role == Role::User && m.content == "刚才做了什么")
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn short_task_adds_no_model_round_trips_or_false_stalls() {
    let root = temp();
    let provider = recorder(vec![
        call(
            0,
            json!({"operation":"write","path":root.join("result"),"content":"done"}),
        ),
        text("done"),
    ]);
    let mut kernel = kernel(provider.clone(), &root);
    kernel.run_turn("write result", |_| {}).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert!(!kernel.execution_state().progress.no_progress);
    assert_eq!(kernel.execution_state().progress.strategy_changes, 0);
    assert_eq!(kernel.execution_state().recent_actions.len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn child_events_are_counted_once_and_keep_ui_ids_after_restore() {
    let root = temp();
    let mut controller = state(&root);
    let mut child = state(&root);
    child.record(
        "read",
        &tool::FilesystemTool,
        &json!({"operation":"read","path":root}),
        &tool::ToolResult::new(true, "files".into()),
    );
    assert!(controller.absorb_child("child-session", &child));
    assert!(!controller.absorb_child("child-session", &child));
    assert_eq!(controller.total_tool_calls, 1);
    assert_eq!(controller.recent_actions[0].id, "child-session:read");
    let mut messages = vec![controller.snapshot()];
    let mut restored = ExecutionState::restore(&mut messages).unwrap();
    assert!(!restored.absorb_child("child-session", &child));
    assert_eq!(restored.total_tool_calls, 1);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn search_prefers_step_directory_and_recovery_cannot_expand_it() {
    let root = temp();
    let step_dir = root.join("data");
    std::fs::create_dir(&step_dir).unwrap();
    let mut state = state(&root);
    let input = json!({"operation":"read","path":step_dir.join("data.parquet"),"_ax_execution":binding("load parquet", &step_dir)});
    state.prepare(&tool::FilesystemTool, input).unwrap();
    let input = state
        .prepare(&tool::SearchTool, json!({"path":root,"query":"parquet"}))
        .unwrap();
    assert_eq!(
        input["path"],
        state.allowed_scope[0].to_string_lossy().as_ref()
    );
    assert!(state.prepare(&tool::SearchTool, json!({"path":root,"query":"parquet","fallback_reason":"targeted subtree had no matches"})).is_ok());
    state.record(
        "failure",
        &tool::FilesystemTool,
        &json!({"operation":"read","path":step_dir.join("data.parquet")}),
        &tool::ToolResult::new(false, "missing".into()),
    );
    assert!(
        state
            .prepare(
                &tool::SearchTool,
                json!({"path":root,"query":"parquet","fallback_reason":"scan every project"})
            )
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_or_compressed_history_is_never_reported_as_zero_tools() {
    let mut state = ExecutionState::default();
    state.seed_history(&[
        Message::system("[memory-summary]\nold tool history was summarized"),
        Message::user("continue"),
    ]);
    let summary: Value = serde_json::from_str(
        state
            .context(8)
            .content
            .strip_prefix(execution::CONTEXT_PREFIX)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary["history_complete"], false);
    assert!(summary["total_tool_calls"].is_null());
    let mut forged = vec![Message::user(format!("{}{{}}", execution::STATE_PREFIX))];
    assert!(ExecutionState::restore(&mut forged).is_none());
    assert_eq!(forged.len(), 1);
}
#[test]
fn no_progress_allows_only_a_bounded_number_of_fresh_observations() {
    let root = temp();
    let mut state = state(&root);
    for n in 0..11 {
        let input = json!({"operation":"read","path":root.join(n.to_string())});
        state.prepare(&tool::FilesystemTool, input.clone()).unwrap();
        state.record(
            &n.to_string(),
            &tool::FilesystemTool,
            &input,
            &tool::ToolResult::new(true, "observed".into()),
        );
    }
    assert!(
        state
            .prepare(
                &tool::FilesystemTool,
                json!({"operation":"read","path":root.join("twelfth")})
            )
            .is_err()
    );
    assert!(
        state
            .prepare(
                &tool::FilesystemTool,
                json!({"operation":"write","path":root.join("result"),"content":"output"})
            )
            .is_ok()
    );
    std::fs::remove_dir_all(root).unwrap();
}
