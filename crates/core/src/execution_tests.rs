use super::*;
use async_trait::async_trait;
use model::{
    FunctionCall, Message, ModelError, ModelProvider, ModelRequest, ModelResponse, Role, ToolCall,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tool::ToolRegistry;

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
fn repeated_failures_recommend_strategy_change_without_blocking_retry_or_diagnosis() {
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
    assert!(state.prepare(&tool::FilesystemTool, input).is_ok());
    assert!(state.progress.no_progress);
    assert_eq!(state.progress.strategy_changes, 1);
    assert!(
        state
            .prepare(&tool::ShellTool, json!({"command":"pwd"}))
            .is_ok()
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
    assert!(state.prepare(&tool::FilesystemTool, json!({"operation":"list","path":root,"_ax_execution":binding("new step without progress", &root)})).is_ok());
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
    let observed = kernel
        .raw_turn_messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("call-8"))
        .unwrap();
    let result: tool::ToolResult = serde_json::from_str(&observed.content).unwrap();
    assert_eq!(result.status, "success");
    assert_eq!(result.raw_output, "dataset");
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
        "ten attempts were recorded, including nine successful reads",
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
fn search_scope_remains_enforced_but_recovery_can_replan_within_workspace() {
    let root = temp();
    let step_dir = root.join("data");
    std::fs::create_dir(&step_dir).unwrap();
    let mut state = state(&root);
    let input = json!({"operation":"read","path":step_dir.join("data.parquet"),"_ax_execution":binding("load parquet", &step_dir)});
    state.prepare(&tool::FilesystemTool, input).unwrap();
    assert!(
        state
            .prepare(
                &tool::SearchTool::default(),
                json!({"path":root,"query":"parquet"})
            )
            .is_err()
    );
    assert!(
        state
            .prepare(
                &tool::SearchTool::default(),
                json!({"path":step_dir,"query":"parquet"})
            )
            .is_ok()
    );
    state.record(
        "failure",
        &tool::FilesystemTool,
        &json!({"operation":"read","path":step_dir.join("data.parquet")}),
        &tool::ToolResult::new(false, "missing".into()),
    );
    assert!(
        state
            .prepare(
                &tool::SearchTool::default(),
                json!({"path":root,"query":"parquet","_ax_execution":binding("locate alternative input", &root)})
            )
            .is_ok()
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
fn no_progress_never_exhausts_observation_admission() {
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
            .is_ok()
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

// Opaque MCP effects retain Resource::All and the scheduler's exclusive lease.
struct EmptyCatalog;
#[async_trait]
impl tool::Tool for EmptyCatalog {
    fn name(&self) -> &'static str {
        "mcp"
    }
    fn description(&self) -> &'static str {
        "Empty catalog fixture"
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object"})
    }
    fn capability(&self, _: &Value) -> tool::Capability {
        tool::Capability::Mcp
    }
    fn safety(&self, _: &Value) -> tool::SafetyLevel {
        tool::SafetyLevel::Safe
    }
    async fn execute(&self, _: Value) -> Result<String, tool::ToolError> {
        Ok("[]".into())
    }
}
fn named_call(n: usize, name: &str, input: Value) -> ModelResponse {
    let mut response = call(n, input);
    response.tool_calls[0].function.name = name.into();
    response
}
fn result_for(kernel: &AgentKernel, n: usize) -> tool::ToolResult {
    let message = kernel
        .raw_turn_messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some(format!("call-{n}").as_str()))
        .unwrap();
    serde_json::from_str(&message.content).unwrap()
}
#[tokio::test]
async fn catalog_empty_observation_allows_new_step_shell_and_list() {
    let root = temp();
    let mut state = state(&root);
    let input = state
        .prepare(
            &EmptyCatalog,
            json!({"action":"catalog","_ax_execution":binding("discover capabilities", &root)}),
        )
        .unwrap();
    state.record(
        "catalog",
        &EmptyCatalog,
        &input,
        &tool::ToolResult::new(true, "[]".into()),
    );
    assert_eq!(state.step_status, execution::StepStatus::Observed);
    assert_eq!(state.progress.advances, 0);
    state.prepare(&tool::ShellTool, json!({"command":"echo ax-diagnostic","_ax_execution":binding("diagnose locally", &root)})).unwrap();
    let provider = recorder(vec![
        named_call(0, "mcp", json!({"action":"catalog"})),
        named_call(1, "shell", json!({"command":"echo ax-diagnostic"})),
        call(2, json!({"operation":"list","path":root})),
        text("done"),
    ]);
    let mut kernel = kernel(provider, &root)
        .with_tool(EmptyCatalog)
        .with_tool(tool::ShellTool);
    kernel
        .run_turn("inspect capabilities and local workspace", |_| {})
        .await
        .unwrap();
    for n in 0..3 {
        assert_eq!(result_for(&kernel, n).status, "success");
    }
    assert_eq!(result_for(&kernel, 0).raw_output, "[]");
    assert!(result_for(&kernel, 1).raw_output.contains("ax-diagnostic"));
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn search_no_match_allows_alternate_search_and_read() {
    let root = temp();
    let file = root.join("input.txt");
    std::fs::write(&file, "needle").unwrap();
    let provider = recorder(vec![
        named_call(0, "search", json!({"path":root,"query":"absent"})),
        named_call(1, "search", json!({"path":root,"query":"needle"})),
        call(2, json!({"operation":"read","path":file})),
        text("done"),
    ]);
    let mut kernel = kernel(provider, &root).with_tool(tool::SearchTool::default());
    kernel.run_turn("locate input", |_| {}).await.unwrap();
    for n in 0..3 {
        assert_eq!(result_for(&kernel, n).status, "success");
    }
    let empty: Value = serde_json::from_str(&result_for(&kernel, 0).raw_output).unwrap();
    assert_eq!(empty["matches"], json!([]));
    let found: Value = serde_json::from_str(&result_for(&kernel, 1).raw_output).unwrap();
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);
    assert_eq!(result_for(&kernel, 2).raw_output, "needle");
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn failure_allows_other_tools_to_diagnose_without_global_recovery_lock() {
    let root = temp();
    let provider = recorder(vec![
        call(0, json!({"operation":"read","path":root.join("missing")})),
        named_call(1, "shell", json!({"command":"echo ax-recovery"})),
        named_call(2, "mcp", json!({"action":"catalog"})),
        call(3, json!({"operation":"list","path":root})),
        text("diagnosed"),
    ]);
    let mut kernel = kernel(provider, &root)
        .with_tool(tool::ShellTool)
        .with_tool(EmptyCatalog);
    kernel
        .run_turn("diagnose missing input", |_| {})
        .await
        .unwrap();
    assert_ne!(result_for(&kernel, 0).status, "success");
    for n in 1..4 {
        assert_eq!(result_for(&kernel, n).status, "success");
    }
    assert!(kernel.execution_state().recovery_for.is_some());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn nonfatal_failures_and_stalls_always_leave_retry_and_diagnostic_tools_available() {
    let root = temp();
    for failed in ["filesystem", "shell", "mcp"] {
        let mut state = state(&root);
        let (tool, input): (&dyn tool::Tool, Value) = match failed {
            "filesystem" => (
                &tool::FilesystemTool,
                json!({"operation":"read","path":root.join("missing")}),
            ),
            "shell" => (&tool::ShellTool, json!({"command":"exit 1"})),
            _ => (&EmptyCatalog, json!({"action":"call"})),
        };
        for n in 0..24 {
            let prepared = state.prepare(tool, input.clone()).unwrap();
            state.record(
                &n.to_string(),
                tool,
                &prepared,
                &tool::ToolResult::new(false, "nonfatal failure".into()),
            );
            assert_eq!(state.step_status, execution::StepStatus::Observed);
            state
                .prepare(
                    &tool::FilesystemTool,
                    json!({"operation":"list","path":root}),
                )
                .unwrap();
            state
                .prepare(
                    &tool::SearchTool::default(),
                    json!({"path":root,"query":"diagnostic"}),
                )
                .unwrap();
            state
                .prepare(&tool::ShellTool, json!({"command":"echo diagnostic"}))
                .unwrap();
            state
                .prepare(&EmptyCatalog, json!({"action":"catalog"}))
                .unwrap();
        }
        assert!(state.progress.no_progress);
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn discovery_tools_cannot_read_outside_the_declared_scope() {
    let root = temp();
    let outside = temp();
    let find = tool::FindFilesTool::default();
    let mut state = state(&root);
    // Both discovery tools declare the resolved path as a read resource, so the
    // runtime rejects a call that points outside the step scope.
    assert!(
        state
            .prepare(
                &tool::SearchTool::default(),
                json!({"query":"needle","path":outside})
            )
            .is_err()
    );
    assert!(
        state
            .prepare(&find, json!({"path":outside,"pattern":"**/*.rs"}))
            .is_err()
    );
    assert!(
        state
            .prepare(
                &tool::SearchTool::default(),
                json!({"query":"needle","path":root})
            )
            .is_ok()
    );
    assert!(
        state
            .prepare(&find, json!({"path":root,"pattern":"**/*.rs"}))
            .is_ok()
    );
    // The rejected calls must not have consumed the scope.
    assert_eq!(state.allowed_scope.len(), 1);
    std::fs::remove_dir_all(outside).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn recovery_does_not_narrow_declared_scope_and_new_step_clears_failed_binding() {
    let root = temp();
    let mut state = state(&root);
    let original_scope = state.allowed_scope.clone();
    let input = state.prepare(&tool::FilesystemTool,json!({"operation":"read","path":root.join("data/missing"),"_ax_execution":binding("load input", &root)})).unwrap();
    state.record(
        "failed",
        &tool::FilesystemTool,
        &input,
        &tool::ToolResult::new(false, "missing".into()),
    );
    assert_eq!(state.allowed_scope, original_scope);
    state.prepare(&tool::FilesystemTool,json!({"operation":"list","path":root,"_ax_execution":binding("next independent task", &root)})).unwrap();
    assert!(state.recovery_for.is_none());
    assert_eq!(state.current_step, "next independent task");
    // Resource boundary remains enforced even during recovery/replanning.
    let outside = root.parent().unwrap();
    assert!(
        state
            .prepare(
                &tool::FilesystemTool,
                json!({"operation":"list","path":outside})
            )
            .is_err()
    );
    assert!(
        state
            .prepare(
                &tool::FilesystemTool,
                json!({"operation":"list","path":outside,"_ax_execution":binding("escape",outside)})
            )
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn legacy_recovery_checkpoint_restores_original_scope_without_tool_lock() {
    let root = temp();
    let mut state = state(&root);
    let input = json!({"operation":"read","path":root.join("missing")});
    state.record(
        "failed",
        &tool::FilesystemTool,
        &input,
        &tool::ToolResult::new(false, "missing".into()),
    );
    let mut legacy = serde_json::to_value(&state).unwrap();
    legacy.as_object_mut().unwrap().remove("step_status");
    legacy["resume_scope"] = json!([root]);
    legacy["allowed_scope"] = json!([root.join("data")]);
    legacy["recovery_attempts"] = json!(100);
    legacy["progress"]["no_progress"] = json!(true);
    let mut restored = ExecutionState::restore(&mut vec![Message::system(format!(
        "{}{legacy}",
        execution::STATE_PREFIX
    ))])
    .unwrap();
    restored
        .prepare(
            &tool::FilesystemTool,
            json!({"operation":"list","path":root}),
        )
        .unwrap();
    restored
        .prepare(&tool::ShellTool, json!({"command":"echo diagnostic"}))
        .unwrap();
    restored
        .prepare(&EmptyCatalog, json!({"action":"catalog"}))
        .unwrap();
    restored.prepare(&tool::FilesystemTool,json!({"operation":"list","path":root,"_ax_execution":binding("first explicit replan", &root)})).unwrap();
    assert!(restored.recovery_for.is_none());
    assert!(restored.recovery_call_id.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn recovery_diagnosis_still_obeys_explicit_permission_denials() {
    let root = temp();
    let provider = recorder(vec![
        call(0, json!({"operation":"read","path":root.join("missing")})),
        named_call(1, "shell", json!({"command":"echo denied"})),
        call(2, json!({"operation":"list","path":root})),
        text("done"),
    ]);
    let mut kernel = kernel(provider, &root).with_tool(tool::ShellTool);
    kernel.permission_profiles.push(tool::PermissionProfile {
        rules: vec![tool::PermissionRule {
            decision: tool::PermissionDecision::Deny,
            matcher: tool::RuleMatcher::ToolParameter {
                tool: "shell".into(),
                pointer: "/command".into(),
                pattern: "*".into(),
            },
        }],
        ..Default::default()
    });
    kernel
        .run_turn("diagnose while respecting policy", |_| {})
        .await
        .unwrap();
    assert_ne!(result_for(&kernel, 0).status, "success");
    assert!(
        result_for(&kernel, 1)
            .raw_output
            .contains("permission denied")
    );
    assert_eq!(result_for(&kernel, 2).status, "success");
    std::fs::remove_dir_all(root).unwrap();
}
