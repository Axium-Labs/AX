//! End-to-end smoke test for the delegated, resumable configuration.
//!
//! One real session, a real child host and a real task queue; only the model is
//! scripted. The scenario is the whole loop the design promises:
//!
//! project rules on disk -> the controller reads them -> it decomposes into
//! three independent children -> the runtime runs them together -> every child
//! returns a structured receipt -> the controller asks the user one question ->
//! the run suspends -> the answer resumes the same goal -> one final answer.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, Role, ToolCall};
use serde_json::json;
use tool::{FilesystemTool, ToolRegistry};

use crate::commands::run::run_prompt_with;
use crate::repl::ReplState;

use runtime_core::{
    AgentEvent, AgentKernel, AllowAll, ApprovalPolicy, ExecutionBudget, QueueState,
    task_queue::TaskStatus,
};

const RULE_TEXT: &str = "Every handler change must be covered by a regression test.";
const ROOT_TEXT: &str = "This repository is AX; keep changes minimal and explain them.";

fn text(content: &str) -> ModelResponse {
    ModelResponse {
        content: content.to_owned(),
        tool_calls: Vec::new(),
        usage: None,
        finish_reason: None,
    }
}

fn call(id: &str, name: &str, input: serde_json::Value) -> ModelResponse {
    ModelResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: id.to_owned(),
            kind: "function".to_owned(),
            function: FunctionCall {
                name: name.to_owned(),
                arguments: input.to_string(),
            },
        }],
        usage: None,
        finish_reason: None,
    }
}

/// Overlap probe for child model rounds.
#[derive(Default)]
struct Observed {
    active: Mutex<usize>,
    peak: AtomicUsize,
}

impl Observed {
    fn enter(&self) {
        let mut active = self.active.lock().unwrap();
        *active += 1;
        let now = *active;
        drop(active);
        self.peak.fetch_max(now, Ordering::SeqCst);
    }

    fn leave(&self) {
        *self.active.lock().unwrap() -= 1;
    }
}

struct Provider {
    requests: Mutex<Vec<ModelRequest>>,
    observed: Arc<Observed>,
}

impl Provider {
    fn controller_requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                !request
                    .messages
                    .iter()
                    .any(|message| message.content.starts_with("[ax-child-runtime]"))
            })
            .cloned()
            .collect()
    }
}

#[async_trait]
impl ModelProvider for Provider {
    fn name(&self) -> &'static str {
        "e2e"
    }
    fn model_id(&self) -> &'static str {
        "e2e"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    fn max_output_tokens(&self) -> Option<usize> {
        Some(1_000)
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        assert!(
            !request
                .messages
                .iter()
                .any(|m| m.content.starts_with("[ax-completion-review]")),
            "default must not invoke reviewer"
        );
        self.requests.lock().unwrap().push(request.clone());
        let child = request
            .messages
            .iter()
            .any(|message| message.content.starts_with("[ax-child-runtime]"));
        if child {
            self.observed.enter();
            tokio::time::sleep(Duration::from_millis(120)).await;
            self.observed.leave();
            let input = request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == Role::User)
                .map_or_else(String::new, |message| message.content.clone());
            let label = input
                .split_whitespace()
                .next_back()
                .unwrap_or("child")
                .to_owned();
            let phase = request
                .messages
                .iter()
                .filter(|message| message.role == Role::Tool)
                .count();
            if phase == 0 {
                return Ok(call(
                    "write",
                    "filesystem",
                    json!({"operation":"write","path":format!("out-{label}.txt"),"content":input}),
                ));
            }
            return Ok(text(&format!("child result: {input}")));
        }
        // Controller: the answer round is the last one.
        let answered = request.messages.iter().any(|message| {
            message
                .tool_call_id
                .as_deref()
                .is_some_and(|id| id.starts_with("question"))
        });
        if answered {
            return Ok(text("Implemented all three changes and verified them."));
        }
        if request
            .messages
            .iter()
            .any(|message| message.content.starts_with("[ax-task-summary]"))
        {
            // Every child is terminal: one planning question, then suspend.
            return Ok(call(
                "question-1",
                "request_user_input",
                json!({
                    "question": "Which format should the report use?",
                    "options": [
                        {"id": "json", "label": "JSON"},
                        {"id": "csv", "label": "CSV"}
                    ]
                }),
            ));
        }
        Ok(call(
            "plan",
            "task_queue",
            json!({
                "action": "start",
                "execution": "children",
                "overall_goal": "refactor the handler and its callers",
                "tasks": [
                    {"title": "api handler", "input": "update the api handler"},
                    {"title": "callers", "input": "update the callers"},
                    {"title": "docs", "input": "update the docs"}
                ]
            }),
        ))
    }
}

struct Fixture {
    root: PathBuf,
    state: ReplState,
    provider: Arc<Provider>,
    observed: Arc<Observed>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ax-e2e-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(project.join("src/api")).unwrap();
        fs::create_dir_all(project.join(".ax/rules")).unwrap();
        fs::write(project.join("AGENTS.md"), ROOT_TEXT).unwrap();
        fs::write(
            project.join("src/AGENTS.md"),
            "Source rules: keep modules focused.",
        )
        .unwrap();
        fs::write(
            project.join(".ax/rules/api.md"),
            format!("---\napplyTo: src/api/**/*.rs\n---\n{RULE_TEXT}"),
        )
        .unwrap();
        fs::write(project.join("src/api/handler.rs"), "fn handle() {}\n").unwrap();

        let mut state =
            ReplState::new_in_project(root.join("data"), root.join("skills"), None, &project)
                .unwrap();
        state.create_session("e2e smoke").unwrap();
        state.allowed_skills = Some(HashSet::new());
        // This fixture has no legacy facts to migrate, so the shared
        // installation-level memory store is never opened: a test must not
        // contend with its siblings over the real global database.
        state.memory_scopes_migrated = true;
        state.execution_budget = ExecutionBudget {
            max_steps: 200,
            max_tool_calls: 200,
            // Unlimited: the idle-window behaviour has its own tests, and this
            // one must not race a wall clock on a loaded machine.
            turn_timeout_secs: 0,
            tool_timeout_secs: 30,
        };
        state.child_timeout_secs = 30;
        let observed = Arc::new(Observed::default());
        let provider = Arc::new(Provider {
            requests: Mutex::new(Vec::new()),
            observed: Arc::clone(&observed),
        });
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(FilesystemTool);
        state.runtime = Some(AgentKernel::new(
            provider.clone(),
            tools,
            Arc::new(AllowAll) as Arc<dyn ApprovalPolicy>,
        ));
        Self {
            root,
            state,
            provider,
            observed,
        }
    }

    fn selection(&self) -> crate::model_selection::ModelSelection {
        crate::model_selection::ModelSelection {
            provider: crate::model_selection::ProviderKind::Compatible,
            provider_id: "e2e-no-network".into(),
            endpoint: None,
            model: "e2e".into(),
            codex_auth: None,
            context_window: Some(100_000),
            max_output_tokens: Some(1_000),
            reasoning_effort: None,
            supports_tools: true,
        }
    }

    async fn run(&mut self, prompt: &str, events: &mut Vec<AgentEvent>) -> String {
        let selection = self.selection();
        let approval: Arc<dyn ApprovalPolicy> = Arc::new(AllowAll);
        run_prompt_with(&mut self.state, &selection, approval, prompt, |event| {
            events.push(event)
        })
        .await
        .expect("prompt completes")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn instructions_then_parallel_children_then_a_question_then_one_final_answer() {
    let mut fixture = Fixture::new();
    let mut events = Vec::new();

    // 1. The controller reads the project rules, decomposes, and dispatches.
    let first = fixture
        .run(
            "Refactor @src/api/handler.rs and report what changed",
            &mut events,
        )
        .await;

    // 2. The run parked on the question instead of ending.
    assert!(
        first.contains("Which format should the report use?"),
        "{first}"
    );
    let question = fixture
        .state
        .pending_question()
        .cloned()
        .expect("a pending question");
    assert_eq!(question.options.len(), 2);
    let goal = fixture
        .state
        .runtime
        .as_ref()
        .and_then(AgentKernel::goal_id)
        .expect("goal")
        .to_owned();

    // 3. Three independent children ran together, each with a receipt.
    {
        let kernel = fixture.state.runtime.as_ref().expect("runtime");
        let queue = kernel.task_queue().expect("queue");
        assert_eq!(queue.goal_id, goal);
        assert_eq!(queue.state, QueueState::WaitingForUser);
        assert_eq!(queue.tasks.len(), 3);
        assert!(
            queue
                .tasks
                .iter()
                .all(|task| task.status == TaskStatus::Completed && task.child.is_some()),
            "{:#?}",
            queue.tasks
        );
        assert_eq!(kernel.child_results().len(), 3);
        for receipt in kernel.child_results().values() {
            assert_eq!(receipt.status, runtime_core::ChildStatus::Completed);
            assert!(
                !receipt.changed_files.is_empty(),
                "a receipt records the files it touched"
            );
            assert!(receipt.metrics.model_rounds >= 1);
            assert!(receipt.metrics.tool_calls >= 1);
            assert!(receipt.summary.contains("child result:"));
        }
        // The task outcome the controller reads is the compact projection, not
        // the full receipt.
        for task in &queue.tasks {
            let outcome = task.outcome.as_deref().unwrap_or_default();
            assert!(outcome.contains("Child "), "{outcome}");
            assert!(outcome.contains("child_result"), "{outcome}");
            assert!(!outcome.contains("diff_stat"), "{outcome}");
        }
    }
    assert!(
        fixture.observed.peak.load(Ordering::SeqCst) >= 2,
        "independent children must overlap"
    );

    // 4. Project instructions reached the controller, with provenance, and the
    //    path-scoped rule applied only because the prompt named a matching file.
    let controller = fixture.provider.controller_requests();
    let plan = controller.first().expect("planning request");
    let first_request = plan
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(first_request.contains(ROOT_TEXT), "root AGENTS.md applied");
    assert!(
        first_request.contains(RULE_TEXT),
        "path-scoped rule applied"
    );
    assert!(first_request.contains("provenance=\"repository root\""));
    assert!(first_request.contains("src/api/**/*.rs"));
    // The directory chain follows the real cwd. This fixture's project root is
    // not an ancestor of the test process cwd, so the nested level is correctly
    // not on the chain and must not leak in; `instructions_tests` covers the
    // root -> cwd chain itself.
    assert!(
        !first_request.contains("Source rules: keep modules focused."),
        "instructions never leak from a directory outside the cwd chain"
    );
    assert!(
        !first_request.contains("[retrieved-memory] all project rules"),
        "instructions are not memory"
    );

    // 5. The user answers; the same goal resumes from the question's position.
    let mut resumed = Vec::new();
    let final_answer = fixture.run("json", &mut resumed).await;
    assert_eq!(
        final_answer,
        "Implemented all three changes and verified them."
    );
    assert!(fixture.state.pending_question().is_none());

    {
        let kernel = fixture.state.runtime.as_ref().expect("runtime");
        let queue = kernel.task_queue().expect("queue");
        assert_eq!(queue.goal_id, goal, "the same goal resumed");
        assert_eq!(queue.state, QueueState::Completed);
        let answer = kernel
            .messages()
            .iter()
            .find(|message| message.tool_call_id.as_deref() == Some("question-1"))
            .expect("the answer was written back to the asking call");
        assert!(answer.content.contains("\"option_id\":\"json\""));
        // Only one user turn was ever opened for the resumed goal.
        assert_eq!(
            kernel
                .messages()
                .iter()
                .filter(|message| message.role == Role::User)
                .count(),
            1
        );
        // The controller saw the compact receipts before answering.
        let summary_request = controller
            .last()
            .expect("summary request")
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(summary_request.contains("child result: update the api handler"));
    }

    // 6. Exactly one final answer was emitted across both runs.
    assert_eq!(
        events
            .iter()
            .chain(resumed.iter())
            .filter(|event| matches!(event, AgentEvent::TurnFinished))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .chain(resumed.iter())
            .filter(|event| matches!(event, AgentEvent::UserQuestion { .. }))
            .count(),
        1
    );
}

/// The user answer must be structured: an unusable answer asks again and leaves
/// the goal waiting rather than failing it.
#[tokio::test]
async fn an_unusable_answer_keeps_the_goal_waiting() {
    let mut fixture = Fixture::new();
    let mut events = Vec::new();
    fixture
        .run("Refactor @src/api/handler.rs", &mut events)
        .await;
    assert!(fixture.state.pending_question().is_some());
    let selection = fixture.selection();
    let approval: Arc<dyn ApprovalPolicy> = Arc::new(AllowAll);
    let error = run_prompt_with(
        &mut fixture.state,
        &selection,
        approval,
        "something else entirely",
        |_| {},
    )
    .await
    .expect_err("an unknown option is rejected");
    assert!(
        format!("{error:#}").contains("json"),
        "the error names the valid answers: {error:#}"
    );
    assert!(fixture.state.pending_question().is_some());
    let queue = fixture
        .state
        .runtime
        .as_ref()
        .unwrap()
        .task_queue()
        .unwrap();
    assert_eq!(queue.state, QueueState::WaitingForUser);
}
