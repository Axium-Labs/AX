//! Parallel-children regression coverage.
//!
//! The controller kernel, the task queue and the child host are all real; only
//! the model and the workspace provisioning are scripted. That keeps the
//! scheduling invariants under test: dependency order, declared-resource
//! conflicts, sibling independence and resume idempotence.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use model::{
    FunctionCall, Message, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall,
};
use serde_json::{Value, json};

use crate::child::{ChildCheckpoint, ChildHost, ChildRun, PreparedChild};
use crate::child_result::{ChildResult, ChildStatus, DiffStat};
use crate::task_queue::TaskStatus;
use crate::{
    AgentError, AgentKernel, AllowAll, ExecutionBudget, GoalTurn, QueueState, child_result,
    task_queue,
};
use tool::{SafetyLevel, Tool, ToolError, ToolRegistry};

const CHILD_ROUND_DELAY: Duration = Duration::from_millis(30);

#[allow(clippy::needless_pass_by_value)] // Mirrors the production call shape.
fn plain(content: &str) -> ModelResponse {
    ModelResponse {
        usage: None,
        content: content.to_owned(),
        tool_calls: Vec::new(),
        finish_reason: None,
    }
}

#[allow(clippy::needless_pass_by_value)] // Mirrors the production call shape.
fn call(id: &str, name: &str, input: Value) -> ModelResponse {
    ModelResponse {
        usage: Some(json!({"prompt_tokens": 10, "completion_tokens": 4})),
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
    }
}

struct Scripted {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl Scripted {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn model_id(&self) -> &'static str {
        "scripted"
    }
    fn context_window(&self) -> usize {
        32_000
    }
    fn max_output_tokens(&self) -> Option<usize> {
        Some(512)
    }
    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ModelError::InvalidResponse("script exhausted".into()))
    }
}

/// Overlap bookkeeping shared by every child's provider.
#[derive(Default)]
struct Observed {
    active: Mutex<Vec<String>>,
    max_overlap: AtomicUsize,
    overlaps: Mutex<Vec<(String, String)>>,
}

impl Observed {
    fn enter(&self, label: &str) {
        let mut active = self.active.lock().unwrap();
        if let Some(other) = active.first() {
            self.overlaps
                .lock()
                .unwrap()
                .push((other.clone(), label.to_owned()));
        }
        active.push(label.to_owned());
        let len = active.len();
        drop(active);
        self.max_overlap.fetch_max(len, Ordering::SeqCst);
    }

    fn exit(&self, label: &str) {
        let mut active = self.active.lock().unwrap();
        if let Some(position) = active.iter().position(|item| item == label) {
            active.remove(position);
        }
    }

    fn overlapped(&self, left: &str, right: &str) -> bool {
        let overlaps = self.overlaps.lock().unwrap();
        overlaps
            .iter()
            .any(|(a, b)| (a == left && b == right) || (a == right && b == left))
    }
}

/// A child provider that announces when it is running, so overlapping children
/// are observable rather than inferred from timing.
struct Tracked {
    label: String,
    observed: Arc<Observed>,
    responses: Mutex<VecDeque<ModelResponse>>,
    round_delay: Duration,
}

#[async_trait]
impl ModelProvider for Tracked {
    fn name(&self) -> &'static str {
        "tracked"
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn model_id(&self) -> &str {
        &self.label
    }
    fn context_window(&self) -> usize {
        32_000
    }
    fn max_output_tokens(&self) -> Option<usize> {
        Some(512)
    }
    async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.observed.enter(&self.label);
        tokio::time::sleep(self.round_delay).await;
        let response = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ModelError::InvalidResponse("script exhausted".into()));
        self.observed.exit(&self.label);
        response
    }
}

struct RecordingCheckpoint {
    receipts: Arc<Mutex<Vec<ChildResult>>>,
}

impl ChildCheckpoint for RecordingCheckpoint {
    fn save(&mut self, _messages: &[Message]) -> Result<(), AgentError> {
        Ok(())
    }
    fn finish(&mut self, result: &mut ChildResult) -> Result<(), AgentError> {
        // The host owns the authoritative diff, as the CLI host does with git.
        result.diff_stat = DiffStat {
            files: result.changed_files.len(),
            insertions: 3,
            deletions: 1,
        };
        self.receipts.lock().unwrap().push(result.clone());
        Ok(())
    }
}

struct MockHost {
    round_delay: Duration,
    prepared: Mutex<Vec<String>>,
    scripts: Mutex<VecDeque<Vec<ModelResponse>>>,
    terminals: Mutex<VecDeque<Option<ChildResult>>>,
    observed: Arc<Observed>,
    receipts: Arc<Mutex<Vec<ChildResult>>>,
    prepared_labels: Mutex<Vec<String>>,
}

impl MockHost {
    fn new(observed: Arc<Observed>, round_delay: Duration) -> Self {
        Self {
            round_delay,
            prepared: Mutex::new(Vec::new()),
            scripts: Mutex::new(VecDeque::new()),
            terminals: Mutex::new(VecDeque::new()),
            observed,
            receipts: Arc::new(Mutex::new(Vec::new())),
            prepared_labels: Mutex::new(Vec::new()),
        }
    }

    fn push_script(&self, responses: Vec<ModelResponse>) {
        self.scripts.lock().unwrap().push_back(responses);
    }

    fn push_terminal(&self, receipt: Option<ChildResult>) {
        self.terminals.lock().unwrap().push_back(receipt);
    }

    fn prepared_labels(&self) -> Vec<String> {
        self.prepared_labels.lock().unwrap().clone()
    }

    fn receipts(&self) -> Vec<ChildResult> {
        self.receipts.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChildHost for MockHost {
    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        _previous: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError> {
        let label = input.lines().next().unwrap_or_default().to_owned();
        self.prepared.lock().unwrap().push(label.clone());
        self.prepared_labels.lock().unwrap().push(label.clone());
        let index = self.prepared.lock().unwrap().len();
        let session = format!("child-{index}");
        let run = ChildRun {
            goal_id: format!("child-{session}"),
            session_id: session,
            cwd: std::env::temp_dir(),
            memory_scope: format!("child:{index}"),
            state_dir: None,
            execution_budget: Some(ExecutionBudget {
                turn_timeout_secs: 5,
                ..ExecutionBudget::default()
            }),
        };
        let mut kernel = controller.fork_child(run.clone(), input, Vec::new());
        kernel.provider = Arc::new(Tracked {
            label,
            round_delay: self.round_delay,
            observed: Arc::clone(&self.observed),
            responses: Mutex::new(
                self.scripts
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| vec![plain("child finished")])
                    .into(),
            ),
        });
        let terminal = self.terminals.lock().unwrap().pop_front().unwrap_or(None);
        if let Some(receipt) = &terminal {
            // The real host closes the receipt while restoring a child that
            // finished in an earlier process.
            self.receipts.lock().unwrap().push(receipt.clone());
        }
        Ok(PreparedChild {
            run,
            kernel,
            checkpoint: Box::new(RecordingCheckpoint {
                receipts: Arc::clone(&self.receipts),
            }),
            terminal,
        })
    }
}

struct AlwaysFail;

#[async_trait]
impl Tool for AlwaysFail {
    fn name(&self) -> &'static str {
        "always_fail"
    }
    fn description(&self) -> &'static str {
        "fails"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn safety(&self, _input: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn capability(&self, _input: &Value) -> tool::Capability {
        tool::Capability::FilesystemRead
    }
    fn resources(&self, _input: &Value) -> Vec<tool::ResourceAccess> {
        Vec::new()
    }
    async fn execute(&self, _input: Value) -> Result<String, ToolError> {
        Err(ToolError::Execution("synthetic child failure".into()))
    }
}

struct Fixture {
    kernel: AgentKernel,
    host: Arc<MockHost>,
    observed: Arc<Observed>,
}

fn harness(controller_script: Vec<ModelResponse>) -> Fixture {
    harness_with(controller_script, CHILD_ROUND_DELAY, 4)
}

fn harness_with(
    controller_script: Vec<ModelResponse>,
    round_delay: Duration,
    concurrency: usize,
) -> Fixture {
    let mut registry = ToolRegistry::new();
    registry.register(AlwaysFail);
    let observed = Arc::new(Observed::default());
    let host = Arc::new(MockHost::new(Arc::clone(&observed), round_delay));
    let kernel = AgentKernel::new(
        Arc::new(Scripted::new(controller_script)),
        registry,
        Arc::new(AllowAll),
    )
    .with_child_host(Arc::clone(&host) as Arc<dyn ChildHost>)
    .with_child_concurrency(concurrency);
    Fixture {
        kernel,
        host,
        observed,
    }
}

#[allow(clippy::needless_pass_by_value)] // JSON literals built by the test.
fn start_queue(kernel: &mut AgentKernel, tasks: Value, dependencies: Value) {
    let goal_id = "goal-parallel";
    kernel.goal_id = Some(goal_id.into());
    let input = json!({
        "action": "start",
        "execution": "children",
        "overall_goal": "exercise parallel children",
        "tasks": tasks,
        "dependencies": dependencies,
    });
    task_queue::apply(&mut kernel.task_queue, &input, goal_id, None).expect("queue accepted");
}

/// A silent event sink. A function pointer keeps the generic parameter concrete
/// so `&Mutex<F>` is unambiguous.
fn emitter() -> Mutex<fn(crate::AgentEvent)> {
    Mutex::new(|_| {})
}

fn titles(kernel: &AgentKernel) -> Vec<(String, String)> {
    kernel
        .task_queue()
        .unwrap()
        .tasks
        .iter()
        .map(|task| (task.title.clone(), format!("{:?}", task.status)))
        .collect()
}

#[tokio::test]
async fn three_independent_tasks_are_dispatched_together() {
    let fixture = harness(vec![]);
    fixture.host.push_script(vec![plain("a done")]);
    fixture.host.push_script(vec![plain("b done")]);
    fixture.host.push_script(vec![plain("c done")]);
    let emit = emitter();
    let mut kernel = fixture.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "read a", "input": "read a"},
            {"title": "read b", "input": "read b"},
            {"title": "read c", "input": "read c"}
        ]),
        json!([[], [], []]),
    );
    let executed = kernel
        .execute_ready_children(&emit, &mut |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(executed, 3);
    assert_eq!(fixture.observed.max_overlap.load(Ordering::SeqCst), 3);
    assert!(
        kernel
            .task_queue()
            .unwrap()
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
    );
    assert_eq!(kernel.task_queue().unwrap().state, QueueState::Summarizing);
    let receipts = fixture.host.receipts();
    assert_eq!(receipts.len(), 3);
    assert!(receipts.iter().all(|receipt| receipt.status.success()));
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.diff_stat.insertions == 3)
    );
}

#[tokio::test]
async fn dependent_tasks_wait_for_their_predecessor() {
    let fixture = harness(vec![]);
    fixture.host.push_script(vec![plain("first done")]);
    fixture.host.push_script(vec![plain("second done")]);
    let emit = emitter();
    let mut kernel = fixture.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "first", "input": "first"},
            {"title": "second", "input": "second"}
        ]),
        json!([[], [0]]),
    );
    let executed = kernel
        .execute_ready_children(&emit, &mut |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(executed, 2);
    // The dependent task cannot start while its predecessor runs.
    assert!(!fixture.observed.overlapped("first", "second"));
    assert_eq!(fixture.host.prepared_labels(), ["first", "second"]);
    assert!(
        kernel
            .task_queue()
            .unwrap()
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
    );
}

#[tokio::test]
async fn read_only_tasks_run_concurrently_and_conflicting_writes_do_not() {
    let fixture = harness(vec![]);
    for label in ["read one", "read two"] {
        fixture.host.push_script(vec![plain(label)]);
    }
    let emit = emitter();
    let mut kernel = fixture.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "read one", "input": "read one", "resources": ["src/api"]},
            {"title": "read two", "input": "read two", "resources": ["src/api"]}
        ]),
        json!([[], []]),
    );
    assert_eq!(
        kernel
            .execute_ready_children(&emit, &mut |_| Ok(()))
            .await
            .unwrap(),
        2
    );
    assert!(fixture.observed.overlapped("read one", "read two"));

    // Same shape, but both tasks declare a write to the same path.
    let writers = harness(vec![]);
    for label in ["write one", "write two"] {
        writers.host.push_script(vec![plain(label)]);
    }
    let mut kernel = writers.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "write one", "input": "write one", "resources": [{"path": "src/api", "write": true}]},
            {"title": "write two", "input": "write two", "resources": [{"path": "src/api/handler.rs", "write": true}]}
        ]),
        json!([[], []]),
    );
    assert_eq!(
        kernel
            .execute_ready_children(&emit, &mut |_| Ok(()))
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        writers.observed.max_overlap.load(Ordering::SeqCst),
        1,
        "conflicting writers must never overlap"
    );
}

#[tokio::test]
async fn a_failed_child_does_not_stop_its_siblings() {
    let fixture = harness(vec![]);
    // Child 1 runs one failing tool, then reports.
    fixture.host.push_script(vec![
        call("f1", "always_fail", json!({})),
        plain("attempted"),
    ]);
    fixture.host.push_script(vec![plain("second ok")]);
    fixture.host.push_script(vec![plain("third ok")]);
    let emit = emitter();
    let mut kernel = fixture.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "boom", "input": "boom"},
            {"title": "ok one", "input": "ok one"},
            {"title": "ok two", "input": "ok two"}
        ]),
        json!([[], [], []]),
    );
    let executed = kernel
        .execute_ready_children(&emit, &mut |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(executed, 3);
    let statuses = titles(&kernel);
    assert!(statuses[0].1.contains("Failed"), "{statuses:?}");
    assert!(statuses[1].1.contains("Completed"), "{statuses:?}");
    assert!(statuses[2].1.contains("Completed"), "{statuses:?}");
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.state, crate::QueueState::Summarizing);
    let summary = queue.tasks[0].outcome.clone().unwrap();
    assert!(summary.starts_with("Child child-1 failed:"));
    // The controller can still reach the full receipt.
    assert!(kernel.child_result("child-1").is_some());
}

#[tokio::test]
async fn resume_never_re_runs_a_completed_child() {
    let fixture = harness(vec![]);
    fixture.host.push_script(vec![plain("second ok")]);
    let emit = emitter();
    let mut kernel = fixture.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "already done", "input": "already done"},
            {"title": "still pending", "input": "still pending"}
        ]),
        json!([[], []]),
    );
    {
        let queue = kernel.task_queue.as_mut().unwrap();
        queue.tasks[0].status = TaskStatus::Completed;
        queue.tasks[0].outcome = Some("Child child-0 completed:".into());
        queue.tasks[1].status = TaskStatus::Pending;
    }
    let executed = kernel
        .execute_ready_children(&emit, &mut |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(executed, 1);
    assert_eq!(fixture.host.prepared_labels(), ["still pending"]);
}

#[tokio::test]
async fn a_recovered_receipt_short_circuits_without_running_the_child() {
    let fixture = harness(vec![]);
    let mut recovered = ChildResult::new("child-recovered", "task-1", ChildStatus::Completed);
    recovered.summary = "finished in an earlier process".into();
    fixture.host.push_terminal(Some(recovered));
    fixture.host.push_script(vec![plain("second ok")]);
    let emit = emitter();
    let mut kernel = fixture.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "recovered", "input": "recovered"},
            {"title": "fresh", "input": "fresh"}
        ]),
        json!([[], []]),
    );
    let executed = kernel
        .execute_ready_children(&emit, &mut |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(executed, 2);
    // Only the second task's model ever ran.
    assert_eq!(fixture.observed.max_overlap.load(Ordering::SeqCst), 1);
    let receipts = fixture.host.receipts();
    assert_eq!(receipts.len(), 2);
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.child_id == "child-recovered")
    );
    assert_eq!(
        kernel
            .task_queue()
            .unwrap()
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .count(),
        2
    );
}

#[tokio::test]
async fn receipts_reach_the_queue_as_a_compact_projection() {
    let fixture = harness(vec![]);
    fixture
        .host
        .push_script(vec![plain("implemented the endpoint")]);
    fixture.host.push_script(vec![plain("wrote the test")]);
    let emit = emitter();
    let mut kernel = fixture.kernel;
    start_queue(
        &mut kernel,
        json!([
            {"title": "implement", "input": "implement"},
            {"title": "test", "input": "test"}
        ]),
        json!([[], []]),
    );
    kernel
        .execute_ready_children(&emit, &mut |_| Ok(()))
        .await
        .unwrap();
    let queue = kernel.task_queue().unwrap();
    for task in &queue.tasks {
        let outcome = task.outcome.as_deref().unwrap();
        assert!(outcome.contains("Child child-"));
        assert!(outcome.contains("- root cause:"));
        assert!(outcome.contains("- validation:"));
        assert!(outcome.contains("child_result"));
    }
    // Full JSON is durable but not part of the conversation.
    let durable = kernel
        .child_results()
        .values()
        .map(|receipt| serde_json::to_string(receipt).unwrap())
        .collect::<Vec<_>>();
    assert!(durable.iter().all(|json| json.contains("diff_stat")));
    assert!(
        !serde_json::to_string(&queue).unwrap().contains("diff_stat"),
        "the queue must not carry full receipts"
    );
    let _ = child_result::TOOL_NAME;
}

#[tokio::test]
async fn user_question_suspends_and_resumes_from_the_same_position() {
    let mut registry = ToolRegistry::new();
    registry.register(AlwaysFail);
    let mut kernel = AgentKernel::new(
        Arc::new(Scripted::new(vec![
            call(
                "ask-1",
                "request_user_input",
                json!({"question": "Ship the migration now?", "options": [{"id": "yes", "label": "Yes"}, {"id": "no", "label": "No"}]}),
            ),
            plain("final answer after the answer"),
        ])),
        registry,
        Arc::new(AllowAll),
    );
    let mut checkpointed = Vec::new();
    let first = kernel
        .run_goal_turn_checkpointed(
            "start the work",
            GoalTurn::New,
            |_| {},
            |saved| {
                checkpointed = saved.to_vec();
                Ok(())
            },
        )
        .await;
    let Err(AgentError::WaitingForUser(question)) = first else {
        panic!("expected a suspension, got {first:?}");
    };
    assert_eq!(question.question, "Ship the migration now?");
    assert!(kernel.pending_question().is_some());
    // The question is durable orchestration state, not conversation: the
    // checkpoint carries the marker so a reconnect can still answer it, while
    // the live transcript the model sees does not.
    assert!(
        checkpointed
            .iter()
            .any(|message| message.content.starts_with(crate::user_input::STATE_PREFIX))
    );
    let mut live = kernel.messages().to_vec();
    assert!(crate::user_input::restore(&mut live).is_none());
    let mut reloaded = kernel.messages().to_vec();
    reloaded.extend(checkpointed.iter().cloned());
    assert!(
        crate::user_input::restore(&mut reloaded).is_some(),
        "a reconnected session must still be waiting for the answer"
    );

    let goal_id = kernel.goal_id().unwrap().to_owned();
    let resume = kernel
        .run_goal_turn(
            "",
            GoalTurn::Answer {
                goal_id,
                answer: crate::UserAnswer::option(question.as_ref(), "yes"),
            },
            |_| {},
        )
        .await
        .expect("resumes after the answer");
    assert_eq!(resume, "final answer after the answer");
    assert!(kernel.pending_question().is_none());
    // The answer landed as the tool result of the call that asked.
    let answer = kernel
        .messages()
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("ask-1"))
        .expect("answer written back to the original call");
    let answer: Value = serde_json::from_str(&answer.content).expect("answer payload");
    assert_eq!(answer["status"], "answered");
    assert_eq!(answer["answer"]["option_id"], "yes");
    assert_eq!(answer["question_id"], format!("q-ask-1"));
    // Exactly one user turn ever opened.
    assert_eq!(
        kernel
            .messages()
            .iter()
            .filter(|message| message.role == model::Role::User)
            .count(),
        1
    );
}

#[tokio::test]
async fn a_productive_child_batch_outlives_the_idle_turn_window() {
    // Six children, two at a time, 700ms each: the batch needs well over the
    // one-second window, but no single stall reaches it. The turn timeout is an
    // idle timeout, so a productive batch must run to completion.
    let fixture = harness_with(
        vec![
            call(
                "plan",
                "task_queue",
                json!({
                    "action": "start",
                    "execution": "children",
                    "overall_goal": "six slow reads",
                    "tasks": (1..=6)
                        .map(|index| json!({"title": format!("slow {index}"), "input": format!("slow {index}")}))
                        .collect::<Vec<_>>(),
                }),
            ),
            plain("all children finished"),
        ],
        Duration::from_millis(700),
        2,
    );
    for index in 1..=6 {
        fixture
            .host
            .push_script(vec![plain(&format!("slow {index}"))]);
    }
    let mut kernel = fixture.kernel.with_execution_budget(ExecutionBudget {
        turn_timeout_secs: 1,
        ..ExecutionBudget::default()
    });
    let outcome = kernel
        .run_goal_turn("dispatch six", GoalTurn::New, |_| {})
        .await
        .expect("a productive batch is not cancelled");
    assert_eq!(outcome, "all children finished");
    assert!(
        kernel
            .task_queue()
            .unwrap()
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed)
    );
}
