use crate::*;
use async_trait::async_trait;
use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

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
    streamed: AtomicUsize,
}
impl Script {
    fn new(replies: Vec<ModelResponse>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(vec![]),
            streamed: AtomicUsize::new(0),
        })
    }
}
#[async_trait]
impl ModelProvider for Script {
    fn name(&self) -> &'static str {
        "continuation-test"
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
            // The callback must already have run before the provider completes.
            self.streamed.fetch_add(1, Ordering::SeqCst);
        }
        Ok(response)
    }
}
struct Operation;
#[async_trait]
impl tool::Tool for Operation {
    fn name(&self) -> &'static str {
        "operation"
    }
    fn description(&self) -> &'static str {
        "fixture for edit/test execution"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object"})
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
fn kernel(provider: Arc<Script>) -> AgentKernel {
    let mut tools = tool::ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(Operation);
    AgentKernel::new(provider, tools, Arc::new(AllowAll)).with_coding_harness()
}

#[tokio::test]
async fn ordinary_answers_and_code_explanations_stream_in_one_request() {
    for input in [
        "hello",
        "Explain the code",
        "Implement a complicated system",
    ] {
        let provider = Script::new(vec![response("answer", vec![])]);
        let mut runtime = kernel(provider.clone());
        let mut events = vec![];
        let result = runtime
            .run_turn(input, |event| {
                if matches!(event, AgentEvent::ContentDelta { .. }) {
                    assert_eq!(provider.streamed.load(Ordering::SeqCst), 0);
                }
                events.push(event);
            })
            .await
            .unwrap();
        assert_eq!(result, "answer");
        assert_eq!(provider.requests.lock().unwrap().len(), 1);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::ContentDelta { .. }))
                .count(),
            1
        );
        assert!(matches!(events.last(), Some(AgentEvent::TurnFinished)));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Completion { completion, model_steps: 1, tools: 0, guard: None, .. } if completion == "direct")));
    }
}

#[tokio::test]
async fn tool_edit_test_and_multiple_rounds_consume_results_without_reviewer() {
    for rounds in [1, 2, 4] {
        let mut replies = (0..rounds)
            .map(|i| {
                response(
                    "",
                    vec![call(&format!("call-{i}"), "operation", json!({"step": i}))],
                )
            })
            .collect::<Vec<_>>();
        replies.push(response("done", vec![]));
        let provider = Script::new(replies);
        kernel(provider.clone())
            .run_turn("edit, test, report", |_| {})
            .await
            .unwrap();
        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), rounds + 1);
        for (index, request) in requests.iter().enumerate().skip(1) {
            assert!(
                request
                    .messages
                    .iter()
                    .any(|m| m.tool_call_id.as_deref() == Some(&format!("call-{}", index - 1)))
            );
            assert!(
                !request
                    .tools
                    .iter()
                    .any(|t| t.function.name == "completion_check")
            );
        }
    }
}

#[test]
fn continuation_is_structural_and_wait_is_distinct() {
    use TurnContinuation::{Continue, Wait};
    for (state, expected) in [
        (
            TurnState {
                pending_tool_calls: 1,
                ..TurnState::default()
            },
            Continue(ContinuationReason::ToolCall),
        ),
        (
            TurnState {
                pending_tool_results: 1,
                ..TurnState::default()
            },
            Continue(ContinuationReason::ToolResult),
        ),
        (
            TurnState {
                pending_tasks: 1,
                ..TurnState::default()
            },
            Continue(ContinuationReason::Task),
        ),
        (
            TurnState {
                running_children: 1,
                ..TurnState::default()
            },
            Wait(WaitReason::Child),
        ),
        (
            TurnState {
                unconsumed_child_results: 1,
                ..TurnState::default()
            },
            Continue(ContinuationReason::ChildResult),
        ),
        (
            TurnState {
                pending_approvals: 1,
                ..TurnState::default()
            },
            Wait(WaitReason::Approval),
        ),
        (
            TurnState {
                pending_user_input: true,
                ..TurnState::default()
            },
            Wait(WaitReason::UserInput),
        ),
        (
            TurnState {
                retry_state: true,
                ..TurnState::default()
            },
            Wait(WaitReason::Retry),
        ),
        (
            TurnState {
                pending_steer: true,
                ..TurnState::default()
            },
            Continue(ContinuationReason::Steer),
        ),
        (
            TurnState {
                required_actions: vec!["execute".into()],
                ..TurnState::default()
            },
            Continue(ContinuationReason::RequiredAction),
        ),
        (
            TurnState {
                model_requests_continuation: true,
                ..TurnState::default()
            },
            Continue(ContinuationReason::Model),
        ),
    ] {
        assert_eq!(state.continuation(), expected);
        assert!(needs_follow_up(&state));
    }
    assert!(!needs_follow_up(&TurnState::default()));
}

#[tokio::test]
async fn pending_task_cannot_finish_and_does_not_invoke_review() {
    let provider = Script::new(vec![
        response(
            "",
            vec![call(
                "queue",
                "task_queue",
                json!({"action":"start", "overall_goal":"work", "tasks":["one", "two"]}),
            )],
        ),
        response("premature", vec![]),
    ]);
    let mut runtime = kernel(provider.clone()).with_execution_budget(ExecutionBudget {
        max_steps: 2,
        ..ExecutionBudget::default()
    });
    let mut finished = false;
    let result = runtime
        .run_turn("work", |e| {
            finished |= matches!(e, AgentEvent::TurnFinished);
        })
        .await;
    assert!(matches!(result, Err(AgentError::StepLimit(2))));
    assert!(!finished);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn deterministic_guard_uses_actual_success_results_and_deliverable_existence() {
    let provider = Script::new(vec![
        response(
            "",
            vec![call("test", "operation", json!({"action":"test"}))],
        ),
        response("done", vec![]),
    ]);
    let mut runtime = kernel(provider.clone());
    runtime.configure_verification(VerificationConfig {
        mode: Verification::Deterministic,
        deliverables: vec![std::env::current_exe().unwrap()],
        required_successful_calls: vec!["test".into()],
    });
    runtime.run_turn("test", |_| {}).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    let guard = crate::stop_guard::DeterministicStopGuard {
        config: VerificationConfig {
            required_successful_calls: vec!["never-ran".into()],
            ..VerificationConfig::default()
        },
    };
    assert!(matches!(
        guard.evaluate(&TurnState::default()).await,
        StopDecision::Continue { .. }
    ));
    let guard = crate::stop_guard::DeterministicStopGuard {
        config: VerificationConfig {
            deliverables: vec![
                std::env::temp_dir().join("ax-nonexistent-continuation-deliverable"),
            ],
            ..VerificationConfig::default()
        },
    };
    assert!(matches!(
        guard.evaluate(&TurnState::default()).await,
        StopDecision::Continue { .. }
    ));
}

#[tokio::test]
async fn only_explicit_model_guard_adds_a_verifier_request() {
    let provider = Script::new(vec![
        response("done", vec![]),
        response(
            "",
            vec![call(
                "guard",
                "stop_decision",
                json!({"allow":true,"reason":"verified"}),
            )],
        ),
    ]);
    let mut runtime = kernel(provider.clone());
    runtime.configure_verification(VerificationConfig {
        mode: Verification::Model,
        ..VerificationConfig::default()
    });
    let mut events = vec![];
    runtime.run_turn("work", |e| events.push(e)).await.unwrap();
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-stop-guard]"))
    );
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Completion { completion, model_steps: 1, guard: Some(guard), .. } if completion == "stop_guard" && guard == "model")));
}

#[tokio::test]
async fn nonfinal_provider_output_continues_without_text_classification() {
    let mut partial = response("I am completely done", vec![]);
    partial.finish_reason = Some("length".into());
    let provider = Script::new(vec![partial, response("final", vec![])]);
    kernel(provider.clone())
        .run_turn("work", |_| {})
        .await
        .unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[test]
fn child_recovery_requires_only_explicit_guard_acceptance() {
    use model::Message;
    let mut history = vec![Message::user("work"), Message::assistant("done", vec![])];
    assert!(terminal_result(&history).is_some());
    history.insert(1, Message::system("[ax-stop-guard-pending]"));
    assert!(terminal_result(&history).is_none());
    history.push(Message::system("[ax-stop-guard-allowed]"));
    assert!(terminal_result(&history).is_some());
}

#[tokio::test]
async fn pending_user_input_waits_then_consumes_answer_without_review() {
    let provider = Script::new(vec![
        response(
            "",
            vec![call(
                "question",
                "request_user_input",
                json!({"question":"Which target?","allow_free_text":true}),
            )],
        ),
        response("done", vec![]),
    ]);
    let mut runtime = kernel(provider.clone());
    let mut finished = 0;
    let result = runtime
        .run_turn("work", |e| {
            if matches!(e, AgentEvent::TurnFinished) {
                finished += 1;
            }
        })
        .await;
    assert!(matches!(result, Err(AgentError::WaitingForUser(_))));
    assert_eq!(finished, 0);
    assert_eq!(
        runtime.turn_state().continuation(),
        TurnContinuation::Wait(WaitReason::UserInput)
    );
    let answer = UserAnswer::free_text(runtime.pending_question().unwrap(), "local");
    let intent = GoalTurn::Answer {
        goal_id: runtime.goal_id().unwrap().into(),
        answer,
    };
    runtime
        .run_goal_turn("local", intent, |_| {})
        .await
        .unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

struct BlockingApproval;
#[async_trait]
impl ApprovalPolicy for BlockingApproval {
    async fn approve(&self, _: &str, _: &serde_json::Value, _: tool::ToolPermission) -> bool {
        std::future::pending().await
    }
}

#[tokio::test]
async fn outstanding_approval_prevents_another_model_request_or_finish() {
    let provider = Script::new(vec![response(
        "",
        vec![call("edit", "operation", json!({}))],
    )]);
    let mut runtime = kernel(provider.clone());
    runtime.approval = Arc::new(BlockingApproval);
    let activity = runtime.activity.clone();
    let mut finished = false;
    {
        let turn = runtime.run_turn("work", |e| {
            finished |= matches!(e, AgentEvent::TurnFinished);
        });
        tokio::pin!(turn);
        tokio::select! {
            _ = &mut turn => panic!("must wait for approval"),
            () = async {
                while activity.approvals.load(Ordering::Acquire) == 0 { tokio::task::yield_now().await; }
            } => {}
        }
        assert_eq!(activity.approvals.load(Ordering::Acquire), 1);
    }
    assert!(!finished);
    assert_eq!(activity.approvals.load(Ordering::Acquire), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

struct RetryThenWait(AtomicUsize);
#[async_trait]
impl ModelProvider for RetryThenWait {
    fn name(&self) -> &'static str {
        "retry"
    }
    fn model_id(&self) -> &'static str {
        "retry"
    }
    fn context_window(&self) -> usize {
        128_000
    }
    async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ModelError> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(ModelError::HttpResponse {
                status: 503,
                message: "retry".into(),
                retry_after: Some(std::time::Duration::ZERO),
            });
        }
        std::future::pending().await
    }
}

#[tokio::test]
async fn unresolved_provider_retry_never_finishes_and_cancellation_clears_state() {
    let mut runtime = AgentKernel::new(
        Arc::new(RetryThenWait(AtomicUsize::new(0))),
        tool::ToolRegistry::default(),
        Arc::new(AllowAll),
    );
    let activity = runtime.activity.clone();
    let mut finished = false;
    {
        let turn = runtime.run_turn("work", |e| {
            finished |= matches!(e, AgentEvent::TurnFinished);
        });
        tokio::pin!(turn);
        tokio::select! {
            _ = &mut turn => panic!("must wait for retry"),
            () = async { while activity.retries.load(Ordering::Acquire) == 0 { tokio::task::yield_now().await; } } => {}
        }
        assert_eq!(activity.retries.load(Ordering::Acquire), 1);
    }
    assert!(!finished);
    assert_eq!(activity.retries.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn guard_denial_continues_and_only_acceptance_emits_finish() {
    let provider = Script::new(vec![
        response("first", vec![]),
        response(
            "",
            vec![call(
                "guard1",
                "stop_decision",
                json!({"allow":false,"reason":"Perform required check"}),
            )],
        ),
        response("", vec![call("check", "operation", json!({}))]),
        response("final", vec![]),
        response(
            "",
            vec![call(
                "guard2",
                "stop_decision",
                json!({"allow":true,"reason":"Check succeeded"}),
            )],
        ),
    ]);
    let mut runtime = kernel(provider.clone());
    runtime.configure_verification(VerificationConfig {
        mode: Verification::Model,
        ..VerificationConfig::default()
    });
    let mut events = vec![];
    assert_eq!(
        runtime.run_turn("work", |e| events.push(e)).await.unwrap(),
        "final"
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 5);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnFinished))
            .count(),
        1
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Completion {
            model_steps: 3,
            guard_model_requests: 2,
            ..
        }
    )));
    let first_delta = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ContentDelta { .. }))
        .unwrap();
    let first_guard = events
        .iter()
        .position(|e| matches!(e, AgentEvent::StopGuardEvaluated { .. }))
        .unwrap();
    assert!(first_delta < first_guard);
}

#[tokio::test]
async fn steer_arriving_during_stream_is_consumed_before_finish() {
    let provider = Script::new(vec![response("first", vec![]), response("updated", vec![])]);
    let mut runtime = kernel(provider.clone());
    let input = runtime.turn_input();
    let mut finished = 0;
    let result = runtime
        .run_turn("work", |event| match event {
            AgentEvent::ContentDelta { delta } if delta == "first" => {
                input.steer("Use the other target");
            }
            AgentEvent::TurnFinished => finished += 1,
            _ => {}
        })
        .await
        .unwrap();
    assert_eq!(result, "updated");
    assert_eq!(finished, 1);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|m| m.role == model::Role::User && m.content == "Use the other target")
    );
}
