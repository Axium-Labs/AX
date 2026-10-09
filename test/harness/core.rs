use crate::*;
use async_trait::async_trait;
use model::{
    FunctionCall, Message, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

struct ReviewProvider {
    round: Mutex<usize>,
}
#[async_trait]
impl ModelProvider for ReviewProvider {
    fn name(&self) -> &'static str {
        "harness-test"
    }
    fn model_id(&self) -> &'static str {
        "fixture"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Configuration(
            "Stream must be set to true".into(),
        ))
    }
    async fn complete_stream(
        &self,
        request: ModelRequest,
        _delta: &mut (dyn FnMut(String) + Send),
        _thinking: &mut (dyn FnMut(String) + Send),
    ) -> Result<ModelResponse, ModelError> {
        let mut round = self.round.lock().unwrap();
        *round += 1;
        assert_eq!(*round, 1, "harness final must not invoke a reviewer");
        assert!(
            !request
                .messages
                .iter()
                .any(|m| m.content.starts_with("[ax-completion-review]"))
        );
        let response = ModelResponse {
            provider_metadata: None,
            content: "direct final".into(),
            tool_calls: vec![],
            usage: None,
            finish_reason: None,
        };
        Ok(response)
    }
}

fn kernel() -> AgentKernel {
    AgentKernel::new(
        Arc::new(ReviewProvider {
            round: Mutex::new(0),
        }),
        tool::ToolRegistry::with_mode(tool::SandboxMode::Off),
        Arc::new(AllowAll),
    )
}

#[tokio::test]
async fn harness_final_is_direct_without_review() {
    let mut kernel = kernel();
    let mut started = 0;
    let mut finished = 0;
    let result = kernel
        .run_turn("Explain this code", |event| match event {
            AgentEvent::ModelStarted { .. } => started += 1,
            AgentEvent::TurnFinished => finished += 1,
            _ => {}
        })
        .await
        .unwrap();
    assert_eq!(result, "direct final");
    assert_eq!((started, finished), (1, 1));
    assert!(!kernel.messages().iter().any(|m| {
        m.content
            .starts_with(crate::child::LEGACY_COMPLETION_PENDING)
    }));
}

#[test]
fn dynamic_typed_inventory_builds_twenty_three_tasks_without_queue_call() {
    let mut runtime = kernel();
    runtime.goal_id = Some("inventory-goal".into());
    runtime.execution.lock().unwrap().begin(
        "execute all discovered items",
        "inventory-goal",
        &std::env::temp_dir(),
    );
    let items=(0..23).map(|i|json!({"title":format!("id-{i}"),"input":format!("fix file-{i}"),"workspace":{"mode":"empty"}})).collect::<Vec<_>>();
    assert!(
        runtime
            .admit_work_items(&json!({"ax_work_items":items}).to_string())
            .unwrap()
    );
    assert_eq!(runtime.task_queue().unwrap().tasks.len(), 23);
    assert!(
        !runtime
            .admit_work_items(&json!({"ax_work_items":items}).to_string())
            .unwrap()
    );
    let mut plain = kernel();
    assert!(
        !plain
            .admit_work_items("1. Rule\n2. Rule\n- pytest unavailable")
            .unwrap()
    );
    assert!(plain.task_queue().is_none());
}

#[tokio::test]
async fn environment_main_and_child_share_capabilities_but_use_own_cwd() {
    let mut main = kernel();
    main.prepare_environment().await.unwrap();
    let main_context = main
        .messages
        .iter()
        .find(|m| m.content.starts_with("[ax-environment]"))
        .unwrap();
    let main_context: serde_json::Value =
        serde_json::from_str(main_context.content.split_once('\n').unwrap().1).unwrap();
    let cwd = std::env::temp_dir();
    let run = ChildRun {
        workspace_root: None,
        goal_id: "child".into(),
        session_id: "child".into(),
        cwd: cwd.clone(),
        memory_scope: "child".into(),
        state_dir: None,
        execution_budget: None,
    };
    let mut child = main.fork_child(run, "fix", vec![Message::user("fix")]);
    child.prepare_environment().await.unwrap();
    let child_context = child
        .messages
        .iter()
        .find(|m| m.content.starts_with("[ax-environment]"))
        .unwrap();
    let child_context: serde_json::Value =
        serde_json::from_str(child_context.content.split_once('\n').unwrap().1).unwrap();
    assert_eq!(main_context["executables"], child_context["executables"]);
    assert_eq!(child_context["cwd"], json!(cwd));
    if cfg!(windows) {
        assert_eq!(child_context["os"], "windows");
        assert!(
            child_context["shell"]
                .as_str()
                .unwrap()
                .contains("PowerShell 5.1")
        );
    }
}

#[test]
fn newly_discovered_work_appends_after_executed_setup_without_losing_history() {
    let mut runtime = kernel();
    runtime.goal_id = Some("expand".into());
    runtime.execution.lock().unwrap().begin(
        "process all input records",
        "expand",
        &std::env::temp_dir(),
    );
    task_queue::apply(&mut runtime.task_queue,&json!({"action":"start","overall_goal":"read data","tasks":["inspect input","inspect output"]}),"expand",None).unwrap();
    task_queue::apply(
        &mut runtime.task_queue,
        &json!({"action":"finish","status":"completed","reason":"setup read done"}),
        "expand",
        None,
    )
    .unwrap();
    task_queue::apply(
        &mut runtime.task_queue,
        &json!({"action":"finish","status":"completed","reason":"output checked"}),
        "expand",
        None,
    )
    .unwrap();
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Summarizing);
    let items=(0..23).map(|id|json!({"title":format!("record {id}"),"input":format!("repair concrete record {id}")})).collect::<Vec<_>>();
    assert!(
        runtime
            .admit_work_items(&json!({"ax_work_items":items}).to_string())
            .unwrap()
    );
    let queue = runtime.task_queue().unwrap();
    assert_eq!(queue.tasks.len(), 25);
    assert_eq!(queue.state, QueueState::Active);
    assert_eq!(queue.tasks[0].status, task_queue::TaskStatus::Completed);
    assert_eq!(queue.tasks[2].status, task_queue::TaskStatus::Running);
    assert!(queue.final_response.is_none());
}

#[test]
fn invented_global_stop_and_local_failure_are_not_global_evidence() {
    let mut runtime = kernel();
    for (id, observation) in [
        ("missing-pytest", "pytest unavailable"),
        ("missing-venv", "venv does not exist"),
        ("missing-runner", "optional local runner absent"),
    ] {
        runtime.messages.push(Message::tool(
            id,
            serde_json::to_string(&tool::ToolResult::new(false, observation.into())).unwrap(),
        ));
        assert!(!runtime.global_stop_evidenced(&json!({"evidence_call_ids":[id]})));
    }
    assert!(!runtime.global_stop_evidenced(&json!({"reason":"environment blocked"})));
    assert!(!runtime.global_stop_evidenced(&json!({"evidence_call_ids":["missing-pytest"]})));
    let mut global = tool::ToolResult::new(false, "shared provider unavailable".into());
    global.global_blocker = Some("provider unavailable".into());
    runtime.messages.push(Message::tool(
        "global",
        serde_json::to_string(&global).unwrap(),
    ));
    assert!(runtime.global_stop_evidenced(&json!({"evidence_call_ids":["global"]})));
}

#[test]
fn coding_step_scope_is_advisory_but_workspace_boundary_is_enforced() {
    let root = std::env::temp_dir().join(format!("ax-advisory-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let runtime = kernel();
    let mut state = runtime.execution.lock().unwrap();
    state.begin("repair code", "goal", &root);
    let input = json!({"operation":"list","path":root,"_ax_execution":{"goal_id":"goal","step":"inspect checkout","scope":["workspace"],"expected_output":"files"}});
    assert!(state.prepare(&tool::FilesystemTool, input).is_ok());
    assert!(
        state
            .prepare(
                &tool::FilesystemTool,
                json!({"operation":"list","path":root})
            )
            .is_ok()
    );
    assert!(
        state
            .prepare(
                &tool::FilesystemTool,
                json!({"operation":"list","path":root.parent()})
            )
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

struct SetupFixture;
#[async_trait]
impl tool::Tool for SetupFixture {
    fn name(&self) -> &'static str {
        "setup_fixture"
    }
    fn description(&self) -> &'static str {
        "Exercise recoverable setup and fallback"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object"})
    }
    fn safety(&self, _: &serde_json::Value) -> tool::SafetyLevel {
        tool::SafetyLevel::Safe
    }
    fn capability(&self, _: &serde_json::Value) -> tool::Capability {
        tool::Capability::FilesystemRead
    }
    async fn execute(&self, input: serde_json::Value) -> Result<String, tool::ToolError> {
        match input["phase"].as_u64().unwrap() {
            0 => Err(tool::ToolError::Execution("pytest unavailable".into())),
            1 => Err(tool::ToolError::Execution("venv missing".into())),
            2 => Err(tool::ToolError::Execution("optional runner missing".into())),
            _ => Ok("fallback local validation completed".into()),
        }
    }
}
struct SetupProvider;
#[async_trait]
impl ModelProvider for SetupProvider {
    fn name(&self) -> &'static str {
        "setup-fixture"
    }
    fn model_id(&self) -> &'static str {
        "fixture"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let review = request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-completion-review]"));
        let phase = request
            .messages
            .iter()
            .filter(|m| m.role == model::Role::Tool)
            .count();
        assert!(!review, "no default review");
        let tool = if phase < 4 {
            Some(("setup_fixture", json!({"phase":phase})))
        } else {
            None
        };
        Ok(ModelResponse {
            provider_metadata: None,
            content: "fallback complete".into(),
            tool_calls: tool
                .into_iter()
                .map(|(name, args)| ToolCall {
                    id: format!("setup-{phase}-{review}"),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: name.into(),
                        arguments: args.to_string(),
                    },
                })
                .collect(),
            usage: None,
            finish_reason: None,
        })
    }
}
#[tokio::test]
async fn recoverable_setup_uses_fallback_without_global_stop_or_user_question() {
    let mut tools = tool::ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(SetupFixture);
    let mut runtime = AgentKernel::new(Arc::new(SetupProvider), tools, Arc::new(AllowAll))
        .with_execution_budget(ExecutionBudget {
            max_steps: 10,
            ..ExecutionBudget::default()
        });
    let mut questions = 0;
    let mut calls = 0;
    runtime
        .run_turn("Repair and locally validate", |event| match event {
            AgentEvent::UserQuestion { .. } => questions += 1,
            AgentEvent::ToolStarted { .. } => calls += 1,
            _ => {}
        })
        .await
        .unwrap();
    assert_eq!(questions, 0);
    assert_eq!(calls, 4);
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Completed);
    assert!(
        runtime
            .messages
            .iter()
            .any(|m| m.content.contains("fallback local validation completed"))
    );
}

#[test]
fn child_completion_review_does_not_hide_tool_failure_or_break_receipt_recovery() {
    let mut messages = vec![
        Message::user("repair"),
        Message::assistant(
            "",
            vec![ToolCall {
                id: "patch".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: "patch".into(),
                    arguments: "{}".into(),
                },
            }],
        ),
        Message::tool(
            "patch",
            serde_json::to_string(&tool::ToolResult::new(false, "patch failed".into())).unwrap(),
        ),
        Message::system(crate::child::LEGACY_COMPLETION_PENDING),
        Message::assistant("could not patch", vec![]),
    ];
    assert!(
        crate::child::terminal_result(&messages).is_none(),
        "a crash before accepted review must resume the child, not fabricate a receipt"
    );
    messages.push(Message::assistant(
        "",
        vec![ToolCall {
            id: "review".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "completion_check".into(),
                arguments: json!({"state":"complete","reason":"local failure evidenced"})
                    .to_string(),
            },
        }],
    ));
    messages.push(Message::tool("review", "Completion review recorded."));
    let outcome = crate::child::terminal_result(&messages).unwrap();
    assert_eq!(outcome.status, ChildStatus::Failed);
    assert!(outcome.failure_reason.unwrap().contains("patch failed"));
    messages[2] = Message::tool(
        "patch",
        serde_json::to_string(&tool::ToolResult::new(true, "patched".into())).unwrap(),
    );
    assert_eq!(
        crate::child::terminal_result(&messages).unwrap().status,
        ChildStatus::Completed
    );
}

struct SummaryReviewProvider;
#[async_trait]
impl ModelProvider for SummaryReviewProvider {
    fn name(&self) -> &'static str {
        "summary-fixture"
    }
    fn model_id(&self) -> &'static str {
        "fixture"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.function.name == "child_result")
        );
        let summary = request
            .messages
            .iter()
            .find(|m| m.content.starts_with("[ax-task-summary]"))
            .expect("model must receive terminal inventory");
        assert!(summary.content.contains("concrete-22"));
        assert!(summary.content.contains("output_dir"));
        assert!(summary.content.contains("total_tasks"));
        assert!(
            summary
                .content
                .contains("Create cross-item reports in the controller")
        );
        assert!(summary.content.contains("authoritative current receipts"));
        let receipts = request
            .messages
            .iter()
            .find(|m| m.content.starts_with("[ax-current-receipts]"))
            .expect("current receipt evidence must reach the model directly");
        assert!(receipts.content.contains("current_output_files"));
        assert!(receipts.content.contains("completed"));
        assert!(summary.content.contains("artifact-manifest.json"));
        assert!(
            summary
                .content
                .contains("never custom result.json status labels")
        );
        Ok(ModelResponse {
            provider_metadata: None,
            content: String::new(),
            tool_calls: vec![],
            usage: None,
            finish_reason: None,
        })
    }
}
#[tokio::test]
async fn model_step_sees_all_terminal_titles_and_durable_output_locations() {
    let mut runtime = kernel();
    runtime.provider = Arc::new(SummaryReviewProvider);
    runtime.child_results.insert(
        "known".into(),
        ChildResult::new("known", "task", ChildStatus::Completed),
    );
    let tasks=(0..23).map(|id|json!({"title":format!("concrete-{id}"),"input":"repair","output_dir":format!("outputs/{id}")})).collect::<Vec<_>>();
    task_queue::apply(
        &mut runtime.task_queue,
        &json!({"action":"start","overall_goal":"repair all records","tasks":tasks}),
        "goal",
        None,
    )
    .unwrap();
    for _ in 0..23 {
        task_queue::apply(
            &mut runtime.task_queue,
            &json!({"action":"finish","status":"completed","reason":"local outcome"}),
            "goal",
            None,
        )
        .unwrap();
    }
    assert_eq!(runtime.task_queue().unwrap().state, QueueState::Summarizing);
    runtime
        .model_step(
            &Mutex::new(|_| {}),
            &[crate::child_result::spec()],
            1,
            &mut |_| Ok(()),
        )
        .await
        .unwrap();
    runtime.task_queue.as_mut().unwrap().state = QueueState::Completed;
    runtime
        .model_step(
            &Mutex::new(|_| {}),
            &[crate::child_result::spec()],
            1,
            &mut |_| Ok(()),
        )
        .await
        .unwrap();
}

#[test]
fn unknown_or_trimmed_model_usage_is_null_and_artifact_writes_are_not_code_edits() {
    let mut observer = crate::child_result::ChildObserver::new();
    for _ in 0..2 {
        observer.observe(&AgentEvent::ModelStarted {
            provider: "fixture".into(),
            model: "fixture".into(),
        });
    }
    let mut message = Message::assistant("one retained round", vec![]);
    message.usage = Some(
        json!({"input_tokens":10,"output_tokens":2,"input_tokens_details":{"cached_tokens":4}}),
    );
    observer.absorb_usage(&[message]);
    observer.observe(&AgentEvent::ToolStarted {
        id: "artifact".into(),
        name: "filesystem".into(),
        detail: String::new(),
        input: json!({"operation":"write","path":".ax-artifacts/result.json"}),
    });
    observer.observe(&AgentEvent::ToolFinished {
        id: "artifact".into(),
        name: "filesystem".into(),
        success: true,
        diagnostics: vec![],
        result: tool::ToolResult::new(true, "saved".into()),
    });
    let receipt = observer.into_result("child", "task", ChildStatus::Completed, None);
    assert_eq!(receipt.metrics.input_tokens, None);
    assert_eq!(receipt.metrics.output_tokens, None);
    assert_eq!(receipt.metrics.cached_input_tokens, None);
    assert_eq!(receipt.metrics.time_to_first_edit_ms, None);
    assert_eq!(receipt.metrics.time_to_first_successful_edit_ms, None);
}
