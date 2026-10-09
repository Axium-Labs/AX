#![allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)]
use super::*;
use async_trait::async_trait;
use model::{
    FunctionCall, Message, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall,
};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use task_queue::{ARCHIVE_PREFIX, PROGRESS_PREFIX, STATE_PREFIX, TaskStatus};
use tool::{SafetyLevel, ToolError, ToolRegistry};

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
        let should_plan = !request
            .messages
            .iter()
            .any(|m| m.content.starts_with(PROGRESS_PREFIX))
            && request
                .messages
                .iter()
                .rev()
                .find(|m| m.role == model::Role::User)
                .is_some_and(|m| task_queue::TaskQueue::list_hints(&m.content).len() >= 2);
        let tasks = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == model::Role::User)
            .map(|m| task_queue::TaskQueue::list_hints(&m.content))
            .unwrap_or_default();
        let goal = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == model::Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        self.requests.lock().unwrap().push(request);
        if should_plan {
            return call(
                "model-plan",
                "task_queue",
                serde_json::json!({"action":"start","overall_goal":goal,"tasks":tasks}),
            );
        }
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected model call")
    }
}
fn text(content: &str) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        provider_metadata: None,
        content: content.into(),
        tool_calls: vec![],
        usage: None,
        finish_reason: None,
    })
}
fn call(id: &str, name: &str, input: Value) -> Result<ModelResponse, ModelError> {
    Ok(ModelResponse {
        provider_metadata: None,
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
fn finish(status: &str, reason: &str) -> Result<ModelResponse, ModelError> {
    call(
        "finish",
        "task_queue",
        serde_json::json!({"action":"finish","status":status,"reason":reason}),
    )
}

/// A fixture whose execution is classified as a runtime global blocker, so a
/// scripted call produces an evidenced `ToolResult.global_blocker`.
struct GlobalBlocker;

#[async_trait]
impl tool::Tool for GlobalBlocker {
    fn name(&self) -> &'static str {
        "blocker"
    }
    fn description(&self) -> &'static str {
        "always fails with a global blocker"
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
    async fn execute(&self, _: Value) -> Result<String, ToolError> {
        Err(ToolError::GlobalBlocked(
            "workspace globally inaccessible".into(),
        ))
    }
}

fn provider(responses: Vec<Result<ModelResponse, ModelError>>) -> Arc<QueueProvider> {
    Arc::new(QueueProvider {
        requests: Mutex::new(vec![]),
        responses: Mutex::new(responses.into()),
    })
}
fn kernel(provider: Arc<QueueProvider>) -> AgentKernel {
    AgentKernel::new(
        provider,
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(AllowAll),
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn twenty_three_tasks_first_fails_remaining_execute_and_only_summary_is_emitted() {
    let executions = Arc::new(Mutex::new(vec![]));
    let mut responses = vec![
        call("bad", "record_task", serde_json::json!({"task":1})),
        call("retry", "record_task", serde_json::json!({"task":1})),
        finish("failed", "environment unavailable after recovery"),
    ];
    for task in 2..=23 {
        responses.push(call(
            &format!("task-{task}"),
            "record_task",
            serde_json::json!({"task":task}),
        ));
        responses.push(finish("completed", "task completed"));
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
    for request in provider.requests.lock().unwrap().iter().skip(1) {
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
        assert_eq!(state.as_object().unwrap().len(), 6);
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
    let mut first = kernel(provider(vec![finish("completed", "first done")]));
    first.budget.max_steps = 2;
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
        Err(AgentError::StepLimit(2))
    ));
    let second_provider = provider(vec![
        finish("completed", "second done"),
        finish("completed", "third done"),
        text("all summarized"),
    ]);
    let mut second = kernel(second_provider.clone()).with_messages(saved);
    assert_eq!(
        second.task_queue().unwrap().tasks[0].status,
        TaskStatus::Completed
    );
    assert_eq!(
        second
            .run_goal_turn(
                "resume",
                GoalTurn::Resume {
                    goal_id: second.goal_id().unwrap().into()
                },
                |_| {}
            )
            .await
            .unwrap(),
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
        call("retry", "hang", serde_json::json!({})),
        finish("failed", "tool timeout after recovery"),
        finish("completed", "second complete"),
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
async fn repeated_provider_failure_blocks_goal_without_fanning_out_over_tasks() {
    let failing_provider = provider(vec![
        Err(ModelError::HttpStatus {
            status: 503,
            message: "temporary".into(),
        }),
        Err(ModelError::HttpStatus {
            status: 503,
            message: "still unavailable".into(),
        }),
    ]);
    let mut runtime = kernel(failing_provider.clone());
    runtime.configure_retry(model::RetryPolicy {
        max_attempts: 2,
        base_delay_ms: 0,
        max_delay_ms: 0,
        time_budget_ms: 1000,
    });
    assert!(
        runtime
            .run_turn("goal\n1. one\n2. two", |_| {})
            .await
            .is_err()
    );
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Blocked);
    assert_eq!(
        runtime.task_queue().unwrap().tasks[1].status,
        TaskStatus::Pending
    );
    assert_eq!(failing_provider.requests.lock().unwrap().len(), 3);
    runtime
        .run_goal_turn(
            "resume",
            GoalTurn::Resume {
                goal_id: runtime.goal_id().unwrap().into(),
            },
            |_| panic!("terminal goal must not emit again"),
        )
        .await
        .unwrap();
    assert_eq!(failing_provider.requests.lock().unwrap().len(), 3);
    let auth_provider = provider(vec![Err(ModelError::HttpStatus {
        status: 401,
        message: "expired credential".into(),
    })]);
    let mut runtime = kernel(auth_provider.clone());
    assert!(matches!(
        runtime.run_turn("goal\n1. one\n2. two", |_| {}).await,
        Err(AgentError::Model(_))
    ));
    assert_eq!(auth_provider.requests.lock().unwrap().len(), 2);
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Blocked);
}

#[test]
fn list_formatting_is_only_a_hint_and_ignores_code() {
    let input = "目标\n1、第一项\n  details\n2、第二项";
    let hints = task_queue::TaskQueue::list_hints(input);
    assert!(hints[0].contains("details"));
    assert_eq!(hints[1], "第二项");
    assert!(task_queue::TaskQueue::list_hints("```\n1. data\n2. data\n```").is_empty());
    // Formatting alone is not a queue: the only producer is an explicit call.
    let queue = task_queue::TaskQueue::new(input.into(), Vec::new());
    assert!(queue.tasks.is_empty());
}

#[tokio::test]
async fn explicit_user_cancel_stops_without_executing_remaining_tasks() {
    let mut runtime = kernel(provider(vec![finish("completed", "one complete")]));
    runtime.budget.max_steps = 2;
    assert!(
        runtime
            .run_turn("Goal\n1. one\n2. two", |_| {})
            .await
            .is_err()
    );
    assert_eq!(
        runtime
            .run_goal_turn(
                "取消长任务",
                GoalTurn::Cancel {
                    goal_id: runtime.goal_id().unwrap().into()
                },
                |_| {}
            )
            .await
            .unwrap(),
        "Task queue canceled by user."
    );
    assert_eq!(
        runtime.task_queue().unwrap().stop_reason.as_deref(),
        Some("Task queue canceled by user.")
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

fn large_queue() -> task_queue::TaskQueue {
    task_queue::TaskQueue::new(
        "Old controller goal".into(),
        (1..=76).map(|i| format!("old task {i}")).collect(),
    )
}

#[tokio::test]
async fn new_user_goal_supersedes_persisted_active_seventy_six_task_queue() {
    let original = large_queue();
    let old_id = original.goal_id.clone();
    let provider = provider(vec![text("New task executed immediately")]);
    let mut runtime = kernel(provider.clone()).with_messages(vec![original.snapshot()]);
    let mut saved = vec![];
    let result = runtime
        .run_turn_checkpointed(
            "New independent user goal",
            |_| {},
            |messages| {
                saved = messages.to_vec();
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(result, "New task executed immediately");
    assert_ne!(runtime.goal_id().unwrap(), old_id);
    assert_eq!(
        runtime.task_queue().unwrap().overall_goal,
        "New independent user goal"
    );
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Completed);
    let archive = saved
        .iter()
        .find(|m| m.content.starts_with(ARCHIVE_PREFIX))
        .unwrap();
    let archived: task_queue::TaskQueue =
        serde_json::from_str(archive.content.strip_prefix(ARCHIVE_PREFIX).unwrap()).unwrap();
    assert_eq!(archived.goal_id, old_id);
    assert_eq!(archived.state, QueueState::Superseded);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let restored = kernel(provider.clone()).with_messages(saved);
    assert_eq!(restored.goal_id(), runtime.goal_id());
    assert!(!restored.task_queue().unwrap().active());
}

#[tokio::test]
async fn pending_work_text_cannot_end_goal_and_global_block_survives_reconnect() {
    let original = large_queue();
    let goal_id = original.goal_id.clone();
    let provider = provider(vec![
        text("Missing runner; stopping"),
        call("hit", "blocker", serde_json::json!({})),
    ]);
    let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(GlobalBlocker);
    let mut runtime = AgentKernel::new(provider.clone(), tools, Arc::new(AllowAll))
        .with_messages(vec![original.snapshot()]);
    let mut saved = vec![];
    let result = runtime
        .run_goal_turn_checkpointed(
            "same goal",
            GoalTurn::Resume {
                goal_id: goal_id.clone(),
            },
            |_| {},
            |m| {
                saved = m.to_vec();
                Ok(())
            },
        )
        .await;
    // Pending work keeps the goal alive after a text-only answer, and a real
    // runtime blocker — not model prose — is what ends it as Blocked.
    assert!(matches!(result, Err(AgentError::GlobalBlocked(_))));
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Blocked);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    let mut resumed = kernel(provider.clone()).with_messages(saved);
    assert_eq!(
        resumed
            .run_goal_turn("resume", GoalTurn::Resume { goal_id }, |_| panic!(
                "terminal state must not emit again"
            ))
            .await
            .unwrap(),
        "global execution blocker: workspace globally inaccessible"
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn explicit_global_stop_does_not_call_model_for_a_summary_or_remaining_tasks() {
    let provider = provider(vec![call("hit", "blocker", serde_json::json!({}))]);
    let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(GlobalBlocker);
    let mut runtime = AgentKernel::new(provider.clone(), tools, Arc::new(AllowAll));
    let result = runtime.run_turn("Goal\n1. one\n2. two", |_| {}).await;
    // A real runtime global blocker — not a model-declared one — ends the goal
    // as Blocked without any summary model call for the remaining tasks.
    assert!(matches!(result, Err(AgentError::GlobalBlocked(_))));
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Blocked);
    // One planning request and the blocked execution; no summary request.
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn new_goal_can_replan_while_mismatched_resume_cannot_touch_old_goal() {
    let original = large_queue();
    let old_id = original.goal_id.clone();
    let provider = provider(vec![
        finish("completed", "new one"),
        finish("completed", "new two"),
        text("new summary"),
    ]);
    let mut runtime = kernel(provider.clone()).with_messages(vec![original.snapshot()]);
    assert!(matches!(
        runtime
            .run_goal_turn(
                "wrong resume",
                GoalTurn::Resume {
                    goal_id: "other-goal".into()
                },
                |_| {}
            )
            .await,
        Err(AgentError::GoalMismatch(_))
    ));
    assert_eq!(runtime.goal_id().unwrap(), old_id);
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Active);
    assert_eq!(provider.requests.lock().unwrap().len(), 0);
    runtime
        .run_turn("New goal\n1. new task one\n2. new task two", |_| {})
        .await
        .unwrap();
    assert_ne!(runtime.goal_id().unwrap(), old_id);
    assert_eq!(runtime.task_queue().unwrap().tasks.len(), 2);
    assert!(
        runtime
            .task_queue()
            .unwrap()
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
    );
}

#[tokio::test]
async fn controller_goal_can_create_isolated_workers_without_new_user_sessions() {
    let original = large_queue();
    let goal_id = original.goal_id.clone();
    let provider = provider(vec![text("worker result"), text("worker result")]);
    let controller = kernel(provider.clone()).with_messages(vec![original.snapshot()]);
    let mut worker = controller.fork_with_messages(vec![
        original.snapshot(),
        Message::user("private worker one"),
    ]);
    assert!(worker.task_queue().is_none());
    assert_eq!(worker.parent_goal_id(), Some(goal_id.as_str()));
    worker.run_turn("worker one", |_| {}).await.unwrap();
    assert_ne!(worker.goal_id(), controller.goal_id());
    let supervisor = AgentSupervisor::new(controller, 2);
    let results = supervisor
        .run_tasks(
            vec![AgentTask {
                id: "worker-two".into(),
                prompt: "worker two".into(),
                context: vec![original.snapshot(), Message::user("private worker two")],
            }],
            None,
        )
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].result.is_ok());
    assert_eq!(
        supervisor
            .template
            .as_ref()
            .unwrap()
            .task_queue()
            .unwrap()
            .state,
        QueueState::Active
    );
    let requests = provider.requests.lock().unwrap();
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|m| m.content == "private worker one")
    );
    assert!(
        !requests[1]
            .messages
            .iter()
            .any(|m| m.content == "private worker one")
    );
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|m| m.content == "private worker two")
    );
    assert!(requests.iter().all(|r| {
        !r.messages
            .iter()
            .any(|m| m.content.starts_with(STATE_PREFIX) || m.content.starts_with(PROGRESS_PREFIX))
    }));
}

struct FatalTool;
#[async_trait]
impl tool::Tool for FatalTool {
    fn name(&self) -> &'static str {
        "fatal"
    }
    fn description(&self) -> &'static str {
        "fatal"
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
    async fn execute(&self, _: Value) -> Result<String, ToolError> {
        Err(ToolError::GlobalBlocked(
            "all workspaces inaccessible".into(),
        ))
    }
}

#[tokio::test]
async fn typed_global_tool_failure_stops_pending_dag_work_and_blocks_goal() {
    let executions = Arc::new(Mutex::new(vec![]));
    let mut response = call("fatal", "fatal", serde_json::json!({})).unwrap();
    response.tool_calls.push(ToolCall {
        id: "dependent".into(),
        kind: "function".into(),
        function: FunctionCall {
            name: "record_task".into(),
            arguments: serde_json::json!({"task":2,"_ax_depends_on":["fatal"]}).to_string(),
        },
    });
    let provider = provider(vec![Ok(response)]);
    let mut runtime = kernel(provider.clone())
        .with_tool(FatalTool)
        .with_tool(RecordingTool(executions.clone()));
    assert!(matches!(
        runtime.run_turn("goal\n1. one\n2. two", |_| {}).await,
        Err(AgentError::GlobalBlocked(_))
    ));
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Blocked);
    assert!(executions.lock().unwrap().is_empty());
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert!(
        runtime
            .messages()
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("dependent"))
    );
}

#[tokio::test]
async fn factual_list_does_not_create_tasks_or_spawn_children() {
    struct ListAnswer;
    #[async_trait]
    impl ModelProvider for ListAnswer {
        fn name(&self) -> &'static str {
            "list-answer"
        }
        fn model_id(&self) -> &'static str {
            "list-answer"
        }
        fn context_window(&self) -> usize {
            32000
        }
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ModelError> {
            text("These are requirements.")
        }
    }
    let mut runtime = AgentKernel::new(
        Arc::new(ListAnswer),
        ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(AllowAll),
    );
    for prompt in [
        "Explain these constraints\n1. memory is local\n2. skills load lazily",
        "Compare:\n- fast\n- slow",
    ] {
        runtime.run_turn(prompt, |_| {}).await.unwrap();
        // The goal queue exists (every goal runs tracked), but a factual list
        // never registers tasks or spawns children.
        let queue = runtime.task_queue().unwrap();
        assert!(queue.tasks.is_empty());
        assert!(!queue.active());
    }
}
#[test]
fn explicit_dependencies_validate_and_failed_dependency_skips_only_dependents() {
    let mut queue = None;
    task_queue::apply(&mut queue,&serde_json::json!({"action":"start","overall_goal":"goal","tasks":["a","b","c"],"dependencies":[[],[0],[]]}),"g",None).unwrap();
    let queue = queue.as_mut().unwrap();
    queue.tasks[0].recovery_attempts = 1;
    queue.finish(TaskStatus::Failed, "failed".into()).unwrap();
    assert_eq!(queue.tasks[1].status, TaskStatus::Skipped);
    assert_eq!(queue.tasks[2].status, TaskStatus::Running);
    assert!(!queue.delegate);
    assert!(task_queue::apply(&mut None,&serde_json::json!({"action":"start","overall_goal":"goal","tasks":["a","b"],"dependencies":[[1],[]]}),"g",None).is_err());
}

#[tokio::test]
async fn failed_task_can_finish_without_forced_recovery_and_next_subtask_executes() {
    let executions = Arc::new(Mutex::new(vec![]));
    let provider = provider(vec![
        call("failed", "record_task", serde_json::json!({"task":1})),
        finish("failed", "unavailable input"),
        call("independent", "record_task", serde_json::json!({"task":2})),
        finish("completed", "done"),
        text("summary"),
    ]);
    let mut kernel = kernel(provider).with_tool(RecordingTool(executions.clone()));
    kernel
        .run_turn("Goal\n1. unavailable task\n2. independent task", |_| {})
        .await
        .unwrap();
    assert_eq!(*executions.lock().unwrap(), vec![1, 2]);
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.tasks[0].status, TaskStatus::Failed);
    assert_eq!(queue.tasks[1].status, TaskStatus::Completed);
}

#[test]
fn unexecuted_legacy_queue_can_replan_but_dispatched_or_terminal_tasks_cannot() {
    let plan =
        |tasks: Value| serde_json::json!({"action":"start","overall_goal":"goal","tasks":tasks});
    let mut queue = None;
    task_queue::apply(
        &mut queue,
        &plan(serde_json::json!(["rule 1", "rule 2"])),
        "g",
        None,
    )
    .unwrap();
    let mut old = serde_json::to_value(queue.as_ref().unwrap()).unwrap();
    for task in old["tasks"].as_array_mut().unwrap() {
        task.as_object_mut().unwrap().remove("input");
        task.as_object_mut().unwrap().remove("execution_started");
    }
    queue = Some(serde_json::from_value(old).unwrap());
    task_queue::apply(
        &mut queue,
        &plan(serde_json::json!([
            {"title":"instance 1","input":"Read dataset.json and process instance 1"},
            {"title":"instance 2","input":"Read dataset.json and process instance 2"}
        ])),
        "g",
        None,
    )
    .unwrap();
    assert_eq!(
        queue.as_ref().unwrap().tasks[0].task_input(),
        "Read dataset.json and process instance 1"
    );
    assert!(
        task_queue::apply(
            &mut queue,
            &plan(serde_json::json!([{"title":"heading only"},{"title":"another heading"}])),
            "g",
            None
        )
        .is_err()
    );
    assert_eq!(queue.as_ref().unwrap().tasks[0].title, "instance 1");
    queue.as_mut().unwrap().tasks[0].execution_started = true;
    assert!(
        task_queue::apply(
            &mut queue,
            &plan(serde_json::json!(["replacement 1", "replacement 2"])),
            "g",
            None
        )
        .is_err()
    );
    let mut legacy = serde_json::to_value(queue.as_ref().unwrap()).unwrap();
    for task in legacy["tasks"].as_array_mut().unwrap() {
        task.as_object_mut().unwrap().remove("input");
        task.as_object_mut().unwrap().remove("execution_started");
    }
    let restored: task_queue::TaskQueue = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.tasks[0].task_input(), "instance 1");
    queue.as_mut().unwrap().tasks[0].execution_started = false;
    queue
        .as_mut()
        .unwrap()
        .finish(TaskStatus::Completed, "already completed".into())
        .unwrap();
    assert!(
        task_queue::apply(
            &mut queue,
            &plan(serde_json::json!(["replacement 1", "replacement 2"])),
            "g",
            None
        )
        .is_err()
    );
}
#[tokio::test]
async fn replan_before_execution_archives_mistaken_queue_and_executes_new_plan() {
    let responses = vec![
        call(
            "mistaken",
            "task_queue",
            serde_json::json!({"action":"start","overall_goal":"goal","tasks":["rule 1","rule 2"]}),
        ),
        call(
            "replan",
            "task_queue",
            serde_json::json!({"action":"start","overall_goal":"goal","tasks":["real task 1","real task 2"]}),
        ),
        finish("completed", "first done"),
        finish("completed", "second done"),
        text("summary"),
    ];
    let mut kernel = kernel(provider(responses));
    kernel
        .run_turn("Process actual independent tasks", |_| {})
        .await
        .unwrap();
    assert_eq!(kernel.task_queue().unwrap().tasks[0].title, "real task 1");
    let archive = kernel
        .raw_turn_messages
        .iter()
        .find(|m| m.content.starts_with(ARCHIVE_PREFIX))
        .unwrap();
    let previous: task_queue::TaskQueue =
        serde_json::from_str(archive.content.strip_prefix(ARCHIVE_PREFIX).unwrap()).unwrap();
    assert_eq!(previous.state, QueueState::Superseded);
    assert_eq!(previous.tasks[0].title, "rule 1");
}
