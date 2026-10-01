#![allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)]
use super::*;
use model::{FunctionCall, ModelResponse, ToolCall};
use std::sync::Mutex;
use task_queue::{PROGRESS_PREFIX, STATE_PREFIX, TaskStatus};

struct QueueProvider {
    requests: Mutex<Vec<ModelRequest>>,
    responses: Mutex<VecDeque<Result<ModelResponse, ModelError>>>,
}
#[async_trait]
impl ModelProvider for QueueProvider {
    fn name(&self) -> &'static str {
        "queue-test"
    }
    fn model_id(&self) -> &'static str {
        "queue-test"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected model call")
    }
}
fn text(content: &str) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        content: content.into(),
        tool_calls: vec![],
        usage: None,
        finish_reason: None,
    })
}
fn call(id: &str, name: &str, input: Value) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: input.to_string(),
            },
        }],
        usage: None,
        finish_reason: None,
    })
}
fn provider(responses: Vec<Result<ModelResponse, ModelError>>) -> Arc<QueueProvider> {
    Arc::new(QueueProvider {
        requests: Mutex::new(vec![]),
        responses: Mutex::new(responses.into()),
    })
}
fn kernel(provider: Arc<QueueProvider>) -> AgentKernel {
    AgentKernel::new(provider, ToolRegistry::new(), Arc::new(AllowAll))
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn twenty_three_tasks_first_fails_remaining_execute_and_only_summary_is_emitted() {
    let executions = Arc::new(Mutex::new(vec![]));
    let mut responses = vec![
        call("bad", "record_task", serde_json::json!({"task":1})),
        text("cannot do task 1"), // runtime requests recovery instead of finishing
        call("retry", "record_task", serde_json::json!({"task":1})),
        text("unrecoverable task 1"),
    ];
    for task in 2..=23 {
        responses.push(call(
            &format!("task-{task}"),
            "record_task",
            serde_json::json!({"task":task}),
        ));
        responses.push(text("task completed"));
    }
    responses.push(text("Final: 22 completed; task 1 failed"));
    let provider = provider(responses);
    let mut kernel = kernel(provider.clone()).with_tool(RecordingTool(executions.clone()));
    let mut emitted = vec![];
    let mut saved = vec![];
    let input = format!(
        "Complete independent tasks\n{}",
        (1..=23)
            .map(|i| format!("{i}. task {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let result = kernel
        .run_turn_checkpointed(
            input,
            |e| emitted.push(e),
            |messages| {
                saved = messages.to_vec();
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(result, "Final: 22 completed; task 1 failed");
    assert_eq!(
        *executions.lock().unwrap(),
        [vec![1, 1], (2..=23).collect()].concat()
    );
    let queue = kernel.task_queue().unwrap();
    assert!(queue.summarized);
    assert_eq!(queue.tasks[0].status, TaskStatus::Failed);
    assert!(
        queue.tasks[0]
            .failure_reason
            .as_ref()
            .unwrap()
            .contains("environment unavailable")
    );
    assert!(
        queue.tasks[1..]
            .iter()
            .all(|t| t.status == TaskStatus::Completed)
    );
    assert_eq!(
        emitted
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnFinished))
            .count(),
        1
    );
    let deltas = emitted
        .iter()
        .filter_map(|e| {
            if let AgentEvent::ContentDelta { delta } = e {
                Some(delta.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(deltas, [result.as_str()]);
    for request in provider.requests.lock().unwrap().iter() {
        assert_eq!(
            request
                .messages
                .iter()
                .filter(|m| m.content.starts_with(PROGRESS_PREFIX))
                .count(),
            1
        );
        assert!(
            !request
                .messages
                .iter()
                .any(|m| m.content.starts_with(STATE_PREFIX))
        );
        let state: Value = serde_json::from_str(
            request
                .messages
                .iter()
                .find(|m| m.content.starts_with(PROGRESS_PREFIX))
                .unwrap()
                .content
                .strip_prefix(PROGRESS_PREFIX)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(state.as_object().unwrap().len(), 5);
    }
    let latest = saved
        .iter()
        .rev()
        .find(|m| m.content.starts_with(STATE_PREFIX))
        .unwrap();
    assert!(latest.content.contains("\"summarized\":true"));
}

#[tokio::test]
async fn reconnect_keeps_completed_tasks_and_resumes_original_queue() {
    let mut first = kernel(provider(vec![text("first done")]));
    first.budget.max_steps = 1;
    let mut saved = vec![];
    assert!(matches!(
        first
            .run_turn_checkpointed(
                "Original goal\n1. first\n2. second\n3. third",
                |_| {},
                |m| {
                    saved = m.to_vec();
                    Ok(())
                }
            )
            .await,
        Err(AgentError::StepLimit(1))
    ));
    let second_provider = provider(vec![
        text("second done"),
        text("third done"),
        text("all summarized"),
    ]);
    let mut second = kernel(second_provider.clone()).with_messages(saved);
    assert_eq!(
        second.task_queue().unwrap().tasks[0].status,
        TaskStatus::Completed
    );
    assert_eq!(
        second.run_turn("resume", |_| {}).await.unwrap(),
        "all summarized"
    );
    let requests = second_provider.requests.lock().unwrap();
    let progress = requests[0]
        .messages
        .iter()
        .find(|m| m.content.starts_with(PROGRESS_PREFIX))
        .unwrap();
    assert!(progress.content.contains("Original goal"));
    assert!(progress.content.contains("\"current_task\":\"second\""));
    assert!(
        second
            .task_queue()
            .unwrap()
            .tasks
            .iter()
            .all(|t| t.status == TaskStatus::Completed)
    );
}

struct HangingTool;
#[async_trait]
impl tool::Tool for HangingTool {
    fn name(&self) -> &'static str {
        "hang"
    }
    fn description(&self) -> &'static str {
        "hang"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type":"object"})
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn capability(&self, _: &Value) -> tool::Capability {
        tool::Capability::FilesystemRead
    }
    fn resources(&self, _: &Value) -> Vec<tool::ResourceAccess> {
        vec![]
    }
    async fn execute(&self, _: Value) -> Result<String, ToolError> {
        std::future::pending().await
    }
}

#[tokio::test]
async fn task_tool_timeout_recovery_then_continues_other_independent_tasks() {
    let provider = provider(vec![
        call("hang", "hang", serde_json::json!({})),
        text("timed out"),
        call("retry", "hang", serde_json::json!({})),
        text("failed after recovery"),
        text("second complete"),
        text("summary"),
    ]);
    let mut kernel = kernel(provider).with_tool(HangingTool);
    kernel.budget.tool_timeout_secs = 1;
    assert_eq!(
        kernel
            .run_turn("Goal\n1. slow task\n2. independent task", |_| {})
            .await
            .unwrap(),
        "summary"
    );
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.tasks[0].status, TaskStatus::Failed);
    assert!(
        queue.tasks[0]
            .failure_reason
            .as_ref()
            .unwrap()
            .contains("timeout")
    );
    assert_eq!(queue.tasks[1].status, TaskStatus::Completed);
}

#[tokio::test]
async fn semantic_queue_initialization_and_explicit_outcomes() {
    let provider = provider(vec![
        call(
            "start",
            "task_queue",
            serde_json::json!({"action":"start","overall_goal":"original","tasks":["one","two"]}),
        ),
        call(
            "one",
            "task_queue",
            serde_json::json!({"action":"finish","status":"completed","reason":"one result"}),
        ),
        call(
            "two",
            "task_queue",
            serde_json::json!({"action":"finish","status":"skipped","reason":"depends on unavailable user input"}),
        ),
        text("final summary"),
    ]);
    let mut kernel = kernel(provider);
    assert_eq!(
        kernel
            .run_turn("Do one and then two", |_| {})
            .await
            .unwrap(),
        "final summary"
    );
    assert_eq!(
        kernel.task_queue().unwrap().tasks[1].status,
        TaskStatus::Skipped
    );
}

#[tokio::test]
async fn repeated_model_failure_is_local_but_auth_failure_is_global() {
    let failing_provider = provider(vec![
        Err(ModelError::InvalidResponse("temporary".into())),
        Err(ModelError::InvalidResponse("still unavailable".into())),
        text("second done"),
        text("summary"),
    ]);
    let mut runtime = kernel(failing_provider);
    runtime
        .run_turn("goal\n1. one\n2. two", |_| {})
        .await
        .unwrap();
    assert_eq!(
        runtime.task_queue().unwrap().tasks[0].status,
        TaskStatus::Failed
    );
    assert_eq!(
        runtime.task_queue().unwrap().tasks[1].status,
        TaskStatus::Completed
    );
    let mut runtime = kernel(provider(vec![Err(ModelError::HttpStatus {
        status: 401,
        message: "expired credential".into(),
    })]));
    assert!(matches!(
        runtime.run_turn("goal\n1. one\n2. two", |_| {}).await,
        Err(AgentError::Model(_))
    ));
    assert_eq!(
        runtime.task_queue().unwrap().tasks[1].status,
        TaskStatus::Pending
    );
}

#[test]
fn explicit_queue_detection_preserves_multiline_details_and_ignores_code() {
    let queue = task_queue::TaskQueue::from_input("目标\n1、第一项\n  details\n2、第二项").unwrap();
    assert!(queue.tasks[0].title.contains("details"));
    assert_eq!(queue.tasks[1].title, "第二项");
    assert!(task_queue::TaskQueue::from_input("```\n1. data\n2. data\n```").is_none());
}

#[tokio::test]
async fn explicit_user_cancel_stops_without_executing_remaining_tasks() {
    let mut runtime = kernel(provider(vec![text("one complete")]));
    runtime.budget.max_steps = 1;
    assert!(
        runtime
            .run_turn("Goal\n1. one\n2. two", |_| {})
            .await
            .is_err()
    );
    assert_eq!(
        runtime.run_turn("取消长任务", |_| {}).await.unwrap(),
        "Task queue canceled by user."
    );
    assert_eq!(
        runtime.task_queue().unwrap().stop_reason.as_deref(),
        Some("user canceled")
    );
    assert_eq!(
        runtime.task_queue().unwrap().tasks[1].status,
        TaskStatus::Running
    );
}

struct RecordingTool(Arc<Mutex<Vec<u64>>>);
#[async_trait]
impl tool::Tool for RecordingTool {
    fn name(&self) -> &'static str {
        "record_task"
    }
    fn description(&self) -> &'static str {
        "Execute a numbered task"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({"type":"object","properties":{"task":{"type":"integer"}},"required":["task"]})
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn capability(&self, _: &Value) -> tool::Capability {
        tool::Capability::FilesystemRead
    }
    fn resources(&self, _: &Value) -> Vec<tool::ResourceAccess> {
        vec![]
    }
    async fn execute(&self, input: Value) -> Result<String, ToolError> {
        let task = input["task"].as_u64().unwrap();
        self.0.lock().unwrap().push(task);
        if task == 1 {
            Err(ToolError::Execution("environment unavailable".into()))
        } else {
            Ok("executed".into())
        }
    }
}
