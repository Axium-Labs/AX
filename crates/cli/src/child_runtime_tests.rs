use super::*;
use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall};
use runtime_core::task_queue::TaskStatus;
use runtime_core::{AgentEvent, AgentSupervisor, AllowAll, ExecutionBudget, GoalTurn, QueueState};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tool::ToolRegistry;

/// Receipt builder for the `finish()` call sites.
fn receipt(success: bool, output: &str) -> runtime_core::ChildResult {
    let status = if success {
        runtime_core::ChildStatus::Completed
    } else {
        runtime_core::ChildStatus::Failed
    };
    let mut result = runtime_core::ChildResult::new(String::new(), String::new(), status);
    result.summary = output.to_owned();
    result.failure_reason = (!success).then(|| output.to_owned());
    result
}

enum Behavior {
    Normal,
    FailFirst,
    PauseSecond,
    HangFirst,
    SemanticPlan,
}
struct Provider {
    requests: Mutex<Vec<ModelRequest>>,
    behavior: Behavior,
}
#[async_trait]
impl ModelProvider for Provider {
    fn name(&self) -> &'static str {
        "child-test"
    }
    fn model_id(&self) -> &'static str {
        "child-test"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().unwrap().push(request.clone());
        if !request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-child-runtime]"))
        {
            if !request
                .messages
                .iter()
                .any(|m| m.content.starts_with("[ax-task-summary]"))
            {
                // This test model explicitly chooses decomposition and delegation.
                let prompt = request
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.role == model::Role::User)
                    .map_or("", |m| m.content.as_str());
                let tasks = if matches!(self.behavior, Behavior::SemanticPlan) {
                    (1..=23).map(|i| format!("child {i}")).collect::<Vec<_>>()
                } else {
                    runtime_core::task_queue::TaskQueue::list_hints(prompt)
                };
                if tasks.len() >= 2 {
                    return Ok(call(
                        "plan",
                        "task_queue",
                        json!({"action":"start","execution":"children","overall_goal":"independent work","tasks":tasks}),
                    ));
                }
            }
            return Ok(text("all children finished"));
        }
        let users = request
            .messages
            .iter()
            .filter(|m| m.role == model::Role::User)
            .collect::<Vec<_>>();
        assert_eq!(
            users.len(),
            1,
            "only explicit task input, no sibling/controller history"
        );
        let input = &users[0].content;
        assert!(
            request
                .messages
                .iter()
                .all(|m| !m.content.contains("controller secret"))
        );
        if matches!(self.behavior, Behavior::SemanticPlan) {
            return Ok(text(&format!("outcome:{input}")));
        }
        if matches!(self.behavior, Behavior::HangFirst) && input == "child 2" {
            return Ok(text("outcome:child 2"));
        }
        let phase = request
            .messages
            .iter()
            .filter(|m| m.role == model::Role::Tool)
            .count();
        if matches!(self.behavior, Behavior::FailFirst) && input == "child 1" {
            return Err(ModelError::Configuration(
                "child 1 environment unavailable".into(),
            ));
        }
        if (matches!(self.behavior, Behavior::PauseSecond) && input == "child 2" && phase == 1)
            || (matches!(self.behavior, Behavior::HangFirst) && input == "child 1")
        {
            std::future::pending::<()>().await;
        }
        Ok(match phase {
            0 => call(
                "write",
                "filesystem",
                json!({"operation":"write","path":"result.txt","content":input}),
            ),
            1 => call(
                "cwd",
                "shell",
                json!({"command":if cfg!(windows) { "(Get-Location).Path" } else { "pwd" }}),
            ),
            2 => call(
                "remember",
                "memory",
                json!({"action":"set","scope":"session","key":"task","value":input,"evidence":input}),
            ),
            _ => text(&format!("outcome:{input}")),
        })
    }
}
fn text(content: &str) -> ModelResponse {
    ModelResponse {
        provider_metadata: None,
        content: content.into(),
        tool_calls: vec![],
        usage: None,
        finish_reason: None,
    }
}
#[allow(clippy::needless_pass_by_value)]
fn call(id: &str, name: &str, input: serde_json::Value) -> ModelResponse {
    ModelResponse {
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
    }
}
fn provider(fail_first: bool, pause_second: bool, hang_first: bool) -> Arc<Provider> {
    Arc::new(Provider {
        requests: Mutex::new(vec![]),
        behavior: if fail_first {
            Behavior::FailFirst
        } else if pause_second {
            Behavior::PauseSecond
        } else if hang_first {
            Behavior::HangFirst
        } else {
            Behavior::Normal
        },
    })
}
struct Fixture {
    root: PathBuf,
    host: Arc<LocalChildHost>,
}

#[tokio::test]
async fn optional_subagent_uses_local_child_session_and_durable_artifact() {
    let fixture = Fixture::new();
    let provider = provider(false, false, false);
    let mut kernel = fixture.kernel(provider.clone());
    kernel.push_context(model::Message::system("controller secret"));
    kernel.configure_subagents(runtime_core::SubagentConfig {
        enabled: true,
        ..runtime_core::SubagentConfig::default()
    });
    let _events = kernel.prepare_subagents().unwrap();
    let id = kernel
        .spawn_agent("child 1", runtime_core::SpawnOptions::default())
        .unwrap();
    let result = kernel.wait_agent(&id).await;
    assert_eq!(result.status, "completed", "{:?}", result.error);
    assert_eq!(result.summary, "outcome:child 1");
    let state = PathBuf::from(&result.artifacts[0]);
    assert!(state.join("child.sqlite3").exists());
    let store = MemoryStore::open(state.join("child.sqlite3")).unwrap();
    let sessions = store.list_sessions(10, 0).unwrap();
    assert_eq!(sessions.len(), 1);
    let messages = store
        .load_messages(&sessions[0].id, None, u32::MAX)
        .unwrap();
    assert!(
        !messages
            .iter()
            .any(|m| m.content.contains("controller secret"))
    );
    assert!(
        messages
            .iter()
            .any(|m| m.content.contains("outcome:child 1"))
    );
    assert_eq!(
        store
            .scoped_memories(memory::MemoryScope::Session, &sessions[0].id)
            .unwrap()[0]
            .value,
        "child 1"
    );
    assert!(!fixture.host.source.join("result.txt").exists());
    drop(store);
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ax-child-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("project.txt"), "controller baseline").unwrap();
        let host = Arc::new(LocalChildHost {
            sandbox: std::sync::OnceLock::new(),
            policy: WorkspacePolicy::default(),
            source,
            root: root.join("children"),
            excluded: vec![],
        });
        Self { root, host }
    }
    fn kernel(&self, provider: Arc<dyn ModelProvider>) -> AgentKernel {
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(tool::FilesystemTool);
        tools.register(tool::ShellTool);
        tools.register(crate::memory_tool::MemoryTool::default());
        AgentKernel::new(provider, tools, Arc::new(AllowAll)).with_child_host(self.host.clone())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // A child that timed out or hung can still hold a file in its
        // workspace, and `remove_dir_all` then fails with "in use". A panic
        // here would happen while another panic unwinds, which aborts the whole
        // test binary and hides the real failure. Cleanup is best effort.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn input(count: usize) -> String {
    format!(
        "Run independent children\n{}",
        (1..=count)
            .map(|i| format!("{i}. child {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
/// Latest durable queue snapshot carried by a checkpoint.
fn checkpoint_queue(messages: &[Message]) -> Option<runtime_core::task_queue::TaskQueue> {
    messages.iter().rev().find_map(|message| {
        message
            .content
            .strip_prefix(runtime_core::task_queue::STATE_PREFIX)
            .and_then(|json| serde_json::from_str(json).ok())
    })
}

#[allow(clippy::too_many_lines)] // One shared assertion battery for all child batches.
fn check_isolation(kernel: &AgentKernel, provider: &Provider, count: usize) {
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.state, QueueState::Completed);
    let mut sessions = std::collections::HashSet::new();
    let mut dirs = std::collections::HashSet::new();
    let mut scopes = std::collections::HashSet::new();
    for (index, task) in queue.tasks.iter().enumerate() {
        let run = task.child.as_ref().unwrap();
        assert!(sessions.insert(&run.session_id));
        assert!(dirs.insert(&run.cwd));
        assert!(scopes.insert(&run.memory_scope));
        assert!(
            !run.cwd.exists(),
            "completed and failed workspaces are disposable"
        );
        assert!(
            run.state_dir
                .as_ref()
                .unwrap()
                .join("child.sqlite3")
                .exists()
        );
        if task.status == TaskStatus::Failed {
            continue;
        }
        let expected = format!("child {}", index + 1);
        assert!(
            !run.cwd.exists(),
            "terminal child workspace must be removed"
        );
        let store =
            MemoryStore::open(run.state_dir.as_ref().unwrap().join("child.sqlite3")).unwrap();
        assert!(store.session(&run.session_id).unwrap().is_some());
        let memory = store
            .scoped_memories(memory::MemoryScope::Session, &run.session_id)
            .unwrap();
        assert_eq!(memory.len(), 1);
        assert_eq!(memory[0].value, expected);
        assert!(
            store
                .scoped_memories(memory::MemoryScope::Global, "global")
                .unwrap()
                .is_empty()
        );
        let messages = store
            .load_messages(&run.session_id, None, u32::MAX)
            .unwrap();
        let cwd = messages
            .iter()
            .find(|m| m.metadata["tool_call_id"] == "cwd")
            .unwrap_or_else(|| {
                panic!(
                    "task {} lacks its cwd result: {:?}",
                    index,
                    messages
                        .iter()
                        .map(|m| (m.role, m.content.chars().take(80).collect::<String>()))
                        .collect::<Vec<_>>()
                )
            });
        // A child stopped mid-call by a cancel/timeout records an explicit
        // placeholder instead of replaying a call whose side effects are
        // unknown. Concurrent children make that reachable, so it is a valid
        // observation rather than a missing one.
        if !cwd.content.starts_with("Interrupted child call") {
            let result: tool::ToolResult =
                serde_json::from_str(&cwd.content).unwrap_or_else(|error| {
                    panic!(
                        "task {index} cwd result unreadable ({error}): role={:?} kind={:?} content={:?}",
                        cwd.role,
                        cwd.kind,
                        cwd.content.chars().take(200).collect::<String>()
                    )
                });
            assert!(
                result
                    .raw_output
                    .contains(&run.cwd.to_string_lossy().to_string())
            );
        }
        assert_eq!(
            messages
                .iter()
                .filter(|m| m.role == memory::MessageRole::User)
                .count(),
            1
        );
    }
    assert_eq!(sessions.len(), count);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| !r
                .messages
                .iter()
                .any(|m| m.content.starts_with("[ax-child-runtime]")))
            .count(),
        if requests.iter().any(|r| !r
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-child-runtime]")
                || m.content.starts_with("[ax-task-summary]")))
        {
            2
        } else {
            1
        }
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One end-to-end batch assertion sequence.
async fn twenty_three_explicitly_delegated_children_execute_in_isolated_contexts_and_finish_once() {
    let fixture = Fixture::new();
    let provider = provider(false, false, false);
    let mut kernel = fixture.kernel(provider.clone());
    kernel.push_context(Message::user("controller secret: never pass to children"));
    kernel.push_context(Message::system("[retrieved-memory] controller secret"));
    let mut events = vec![];
    assert_eq!(
        kernel
            .run_turn(input(23), |event| events.push(event))
            .await
            .unwrap(),
        "all children finished"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ContentDelta { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnFinished))
            .count(),
        1
    );
    assert!(
        kernel
            .task_queue()
            .unwrap()
            .tasks
            .iter()
            .all(|t| t.status == TaskStatus::Completed),
        "{:#?}",
        kernel.task_queue().unwrap().tasks
    );
    let tool_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolStarted { id, .. } => Some(id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_ids.len(),
        tool_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
    );
    check_isolation(&kernel, &provider, 23);
    assert!(!fixture.host.source.join("result.txt").exists());
}

#[tokio::test]
async fn first_child_failure_does_not_stop_twenty_two_independent_children() {
    let fixture = Fixture::new();
    let provider = provider(true, false, false);
    let mut kernel = fixture.kernel(provider.clone());
    kernel.run_turn(input(23), |_| {}).await.unwrap();
    assert_eq!(
        kernel.task_queue().unwrap().tasks[0].status,
        TaskStatus::Failed
    );
    assert!(
        kernel.task_queue().unwrap().tasks[0]
            .failure_reason
            .as_ref()
            .unwrap()
            .contains("environment unavailable")
    );
    assert!(
        kernel.task_queue().unwrap().tasks[1..]
            .iter()
            .all(|t| t.status == TaskStatus::Completed),
        "{:#?}",
        kernel.task_queue().unwrap().tasks
    );
    check_isolation(&kernel, &provider, 23);
}

#[tokio::test]
async fn reconnect_resumes_running_child_history_and_remaining_queue_position() {
    let fixture = Fixture::new();
    let first_provider = provider(false, true, false);
    let mut first = fixture.kernel(first_provider);
    let saved = Arc::new(Mutex::new(vec![]));
    let saved_callback = saved.clone();
    // Wait for the observable condition -- child 1 terminal while child 2 is
    // stalled -- instead of a wall-clock deadline: 23 concurrent children make
    // a fixed deadline flaky when the suite runs tests in parallel.
    {
        let turn = first.run_turn_checkpointed(
            input(23),
            |_| {},
            |messages| {
                *saved_callback.lock().unwrap() = messages.to_vec();
                Ok(())
            },
        );
        tokio::pin!(turn);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut finished = false;
        loop {
            if tokio::time::timeout(std::time::Duration::from_millis(20), &mut turn)
                .await
                .is_ok()
            {
                finished = true;
                break;
            }
            let terminal = checkpoint_queue(&saved.lock().unwrap())
                .is_some_and(|queue| queue.tasks[0].status == TaskStatus::Completed);
            if terminal {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child 1 never reached a terminal state"
            );
        }
        assert!(
            !finished,
            "a stalled child must keep the turn open, not end it"
        );
        // Leaving the block is what a disconnect does: every in-flight child is
        // dropped with the turn.
    }
    let queue = first.task_queue().unwrap();
    assert_eq!(
        queue.tasks[0].status,
        TaskStatus::Completed,
        "{:#?}",
        queue.tasks
    );
    assert_eq!(queue.tasks[1].status, TaskStatus::Running);
    let second_run = queue.tasks[1].child.clone().unwrap();
    assert!(second_run.cwd.exists());
    assert!(!queue.tasks[0].child.as_ref().unwrap().cwd.exists());
    let goal = queue.goal_id.clone();
    drop(first);
    let resumed_provider = provider(false, false, false);
    let mut restored = fixture
        .kernel(resumed_provider.clone())
        .with_messages(saved.lock().unwrap().clone());
    restored
        .run_goal_turn("continue", GoalTurn::Resume { goal_id: goal }, |_| {})
        .await
        .unwrap();
    assert_eq!(
        restored.task_queue().unwrap().tasks[1].child.as_ref(),
        Some(&second_run)
    );
    assert!(resumed_provider.requests.lock().unwrap().iter().all(|r| {
        !r.messages
            .iter()
            .any(|m| m.role == model::Role::User && m.content == "child 1")
    }));
    check_isolation(&restored, &resumed_provider, 23);
}

#[tokio::test]
async fn child_timeout_ends_only_that_child() {
    let fixture = Fixture::new();
    let provider = provider(false, false, true);
    let mut kernel = fixture
        .kernel(provider.clone())
        .with_child_execution_budget(ExecutionBudget {
            turn_timeout_secs: 1,
            ..ExecutionBudget::default()
        });
    kernel.run_turn(input(2), |_| {}).await.unwrap();
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.tasks[0].status, TaskStatus::Failed);
    assert!(
        queue.tasks[0]
            .failure_reason
            .as_ref()
            .unwrap()
            .contains("child execution timeout")
    );
    assert_eq!(queue.tasks[1].status, TaskStatus::Completed);
    assert_eq!(
        provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| !r
                .messages
                .iter()
                .any(|m| m.content.starts_with("[ax-child-runtime]")))
            .count(),
        2
    );
}

#[tokio::test]
async fn durable_child_receipt_prevents_reexecution_after_controller_checkpoint_failure() {
    let fixture = Fixture::new();
    let mut first = fixture.kernel(provider(false, false, false));
    let mut saved = vec![];
    let result = first
        .run_turn_checkpointed(
            input(2),
            |_| {},
            |messages| {
                let queue = messages
                    .iter()
                    .rev()
                    .find_map(|m| {
                        m.content
                            .strip_prefix(runtime_core::task_queue::STATE_PREFIX)
                    })
                    .map(|s| {
                        serde_json::from_str::<runtime_core::task_queue::TaskQueue>(s).unwrap()
                    });
                if queue
                    .as_ref()
                    .is_some_and(|q| q.tasks[0].status == TaskStatus::Completed)
                {
                    return Err(AgentError::Persistence(
                        "simulated disconnect before controller commit".into(),
                    ));
                }
                saved = messages.to_vec();
                Ok(())
            },
        )
        .await;
    assert!(result.is_err());
    let goal = first.goal_id().unwrap().to_owned();
    let resumed_provider = provider(false, false, false);
    let mut restored = fixture
        .kernel(resumed_provider.clone())
        .with_messages(saved);
    restored
        .run_goal_turn("continue", GoalTurn::Resume { goal_id: goal }, |_| {})
        .await
        .unwrap();
    assert!(resumed_provider.requests.lock().unwrap().iter().all(|r| {
        !r.messages
            .iter()
            .any(|m| m.role == model::Role::User && m.content == "child 1")
    }));
    check_isolation(&restored, &resumed_provider, 2);
}

#[tokio::test]
async fn git_children_have_separate_worktrees_with_controller_working_copy_changes() {
    let fixture = Fixture::new();
    let source = &fixture.host.source;
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "user.name", "AX Test"],
        vec!["add", "project.txt"],
        vec!["commit", "-m", "baseline"],
    ] {
        let args = args.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>();
        assert!(git(source, &args).unwrap().status.success());
    }
    std::fs::write(source.join("project.txt"), "uncommitted controller input").unwrap();
    let controller = fixture.kernel(provider(false, false, false));
    let one = fixture
        .host
        .prepare(&controller, "child 1", None)
        .await
        .unwrap();
    let two = fixture
        .host
        .prepare(&controller, "child 2", None)
        .await
        .unwrap();
    assert_ne!(one.run.cwd, two.run.cwd);
    assert!(one.run.cwd.join(".git").is_file());
    assert!(two.run.cwd.join(".git").is_file());
    assert_eq!(
        std::fs::read_to_string(one.run.cwd.join("project.txt")).unwrap(),
        "uncommitted controller input"
    );
    std::fs::write(one.run.cwd.join("project.txt"), "first child changes").unwrap();
    assert_eq!(
        std::fs::read_to_string(two.run.cwd.join("project.txt")).unwrap(),
        "uncommitted controller input"
    );
    assert_eq!(
        std::fs::read_to_string(source.join("project.txt")).unwrap(),
        "uncommitted controller input"
    );
}

#[tokio::test]
async fn model_created_twenty_three_task_queue_executes_children_without_finish_controls() {
    let fixture = Fixture::new();
    let mut provider = provider(false, false, false);
    Arc::get_mut(&mut provider).unwrap().behavior = Behavior::SemanticPlan;
    let mut kernel = fixture.kernel(provider.clone());
    let mut events = vec![];
    kernel
        .run_turn("Execute independent jobs", |event| events.push(event))
        .await
        .unwrap();
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.tasks.len(), 23);
    assert!(
        queue
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Completed && task.child.is_some()),
        "{:#?}",
        queue.tasks
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::TurnFinished))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ContentDelta { .. }))
            .count(),
        1
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 25); // plan + 23 children + summary
}

struct ToolFailureProvider {
    inner: Arc<Provider>,
}
#[async_trait]
impl ModelProvider for ToolFailureProvider {
    fn name(&self) -> &'static str {
        "tool-failure-test"
    }
    fn model_id(&self) -> &'static str {
        "tool-failure-test"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        if request
            .messages
            .iter()
            .any(|m| m.role == model::Role::User && m.content == "child 1")
        {
            return Ok(
                if request.messages.iter().any(|m| m.role == model::Role::Tool) {
                    text("this child could not complete")
                } else {
                    call("fail", "shell", json!({"command":"exit 7"}))
                },
            );
        }
        self.inner.complete(request).await
    }
}

#[tokio::test]
async fn unresolved_child_tool_failure_is_failed_not_a_successful_text_answer() {
    let fixture = Fixture::new();
    let base = provider(false, false, false);
    let mut kernel = fixture.kernel(Arc::new(ToolFailureProvider { inner: base }));
    kernel.run_turn(input(2), |_| {}).await.unwrap();
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.tasks[0].status, TaskStatus::Failed);
    assert!(
        queue.tasks[0]
            .failure_reason
            .as_ref()
            .unwrap()
            .contains("exit_code: 7")
    );
    assert_eq!(queue.tasks[1].status, TaskStatus::Completed);
}

#[tokio::test]
async fn child_workspace_excludes_controller_store_and_binds_relative_file_paths() {
    use tool::Tool;
    let mut fixture = Fixture::new();
    let database = fixture.host.source.join("memory.sqlite3");
    let sessions = fixture.host.source.join("sessions");
    std::fs::write(&database, "controller private memory").unwrap();
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("sibling.jsonl"), "sibling tool history").unwrap();
    Arc::get_mut(&mut fixture.host).unwrap().excluded = vec![database, sessions];
    let controller = fixture.kernel(provider(false, false, false));
    let child = fixture
        .host
        .prepare(&controller, "isolated files", None)
        .await
        .unwrap();
    assert!(!child.run.cwd.join("memory.sqlite3").exists());
    assert!(!child.run.cwd.join("sessions").exists());
    let context = tool::RunContext {
        workspace_root: child.run.cwd.clone(),
        cwd: child.run.cwd.clone(),
        state_dir: child.run.state_dir.clone().unwrap(),
        session_id: child.run.session_id.clone(),
        memory_scope: child.run.memory_scope.clone(),
        input: "isolated files".into(),
    };
    let filesystem = tool::FilesystemTool.fork_for_run(&context).unwrap();
    // This fixture explicitly uses off mode. Escape assertions live in the
    // Linux runtime_sandbox suite and exercise the actual kernel boundary.
    filesystem
        .execute(json!({"operation":"write","path":"child.txt","content":"owned"}))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(child.run.cwd.join("child.txt")).unwrap(),
        "owned"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.host.source.join("project.txt")).unwrap(),
        "controller baseline"
    );
}

#[tokio::test]
async fn snapshot_honors_nested_gitignore_and_axignore_with_negation() {
    let fixture = Fixture::new();
    let source = &fixture.host.source;
    std::fs::create_dir_all(source.join("nested")).unwrap();
    for (path, content) in [
        (".gitignore", "*.cache\n!keep.cache\n"),
        (".axignore", "private.txt\n"),
        ("nested/.axignore", "private-nested.txt\n"),
        ("private.txt", "secret"),
        ("nested/private-nested.txt", "secret"),
        ("skip.cache", "cache"),
        ("keep.cache", "input"),
        (".hidden-input", "input"),
    ] {
        std::fs::write(source.join(path), content).unwrap();
    }
    let controller = fixture.kernel(provider(false, false, false));
    let child = fixture
        .host
        .prepare(&controller, "filtered", None)
        .await
        .unwrap();
    for path in ["private.txt", "nested/private-nested.txt", "skip.cache"] {
        assert!(!child.run.cwd.join(path).exists(), "{path}");
    }
    assert!(child.run.cwd.join("keep.cache").exists());
    assert!(child.run.cwd.join(".hidden-input").exists());
    assert!(!child.run.cwd.join(".ax").exists());
    assert!(
        child
            .run
            .state_dir
            .as_ref()
            .unwrap()
            .join("child.sqlite3")
            .exists()
    );
}

fn init_git(source: &Path) {
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@example.invalid"],
        vec!["config", "user.name", "AX Test"],
        vec!["add", "."],
        vec!["commit", "-m", "baseline"],
    ] {
        assert!(
            git(
                source,
                &args.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>()
            )
            .unwrap()
            .status
            .success()
        );
    }
}

#[tokio::test]
async fn git_dirty_binary_patch_covers_staged_unstaged_rename_delete_and_untracked() {
    let fixture = Fixture::new();
    let source = &fixture.host.source;
    for (path, content) in [
        ("delete.txt", "gone"),
        ("rename.txt", "renamed"),
        ("private.txt", "private"),
    ] {
        std::fs::write(source.join(path), content).unwrap();
    }
    std::fs::write(source.join("binary.bin"), [0, 1, 2, 0]).unwrap();
    init_git(source);
    std::fs::write(source.join("project.txt"), "staged").unwrap();
    assert!(
        git(source, &["add", "project.txt"].map(std::ffi::OsStr::new))
            .unwrap()
            .status
            .success()
    );
    std::fs::write(source.join("project.txt"), "staged plus unstaged").unwrap();
    assert!(
        git(
            source,
            &["mv", "rename.txt", "renamed.txt"].map(std::ffi::OsStr::new)
        )
        .unwrap()
        .status
        .success()
    );
    std::fs::remove_file(source.join("delete.txt")).unwrap();
    std::fs::write(source.join("binary.bin"), [0, 9, 8, 0, 7]).unwrap();
    std::fs::write(source.join(".gitignore"), "*.cache\n").unwrap();
    std::fs::write(source.join(".axignore"), "private.txt\n").unwrap();
    std::fs::write(source.join("skip.cache"), "cache").unwrap();
    std::fs::write(source.join("new.txt"), "untracked").unwrap();
    let controller = fixture.kernel(provider(false, false, false));
    let mut child = fixture
        .host
        .prepare(&controller, "patch", None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(child.run.cwd.join("binary.bin")).unwrap(),
        [0, 9, 8, 0, 7]
    );
    assert_eq!(
        std::fs::read_to_string(child.run.cwd.join("project.txt")).unwrap(),
        "staged plus unstaged"
    );
    assert!(child.run.cwd.join("renamed.txt").exists());
    for path in ["rename.txt", "delete.txt", "skip.cache", "private.txt"] {
        assert!(!child.run.cwd.join(path).exists(), "{path}");
    }
    assert!(child.run.cwd.join("new.txt").exists());
    let run = child.run.clone();
    child
        .checkpoint
        .finish(&mut receipt(true, "durable result"))
        .unwrap();
    assert!(!run.cwd.exists());
    let worktrees = git(
        source,
        &["worktree", "list", "--porcelain"].map(std::ffi::OsStr::new),
    )
    .unwrap();
    assert!(
        !String::from_utf8_lossy(&worktrees.stdout)
            .contains(&run.cwd.to_string_lossy().replace('\\', "/"))
    );
    drop(child);
    let recovered = fixture
        .host
        .prepare(&controller, "patch", Some(&run))
        .await
        .unwrap();
    assert_eq!(recovered.terminal.unwrap().summary, "durable result");
    assert!(
        !run.cwd.exists(),
        "receipt recovery must not provision again"
    );
}

#[tokio::test]
async fn gc_expires_unleased_interrupted_workspace_but_preserves_history_and_active_leases() {
    let fixture = Fixture::new();
    let controller = fixture.kernel(provider(false, false, false));
    let active = fixture
        .host
        .prepare(&controller, "active", None)
        .await
        .unwrap();
    let abandoned = fixture
        .host
        .prepare(&controller, "abandoned", None)
        .await
        .unwrap();
    let abandoned_run = abandoned.run.clone();
    drop(abandoned);
    for run in [&active.run, &abandoned_run] {
        let state = run.state_dir.as_ref().unwrap();
        let mut manifest = workspace::read_manifest(state).unwrap();
        manifest.touched = 0;
        workspace::write_manifest(state, &manifest).unwrap();
    }
    workspace::gc(
        &fixture.host.root,
        WorkspacePolicy {
            ttl_secs: 1,
            ..fixture.host.policy
        },
    )
    .unwrap();
    assert!(
        active.run.cwd.exists(),
        "active process lease prevents GC even with old heartbeat"
    );
    assert!(!abandoned_run.cwd.exists());
    assert!(
        abandoned_run
            .state_dir
            .as_ref()
            .unwrap()
            .join("child.sqlite3")
            .exists()
    );
    assert_eq!(
        workspace::read_manifest(abandoned_run.state_dir.as_ref().unwrap())
            .unwrap()
            .status,
        "expired"
    );
    let expired = fixture
        .host
        .prepare(&controller, "abandoned", Some(&abandoned_run))
        .await
        .unwrap();
    let outcome = expired.terminal.unwrap();
    assert!(!outcome.status.success());
    assert!(outcome.summary.contains("expired"));
}

#[tokio::test]
async fn quota_rejection_cleans_partial_workspace_and_admission_recovers_after_cleanup() {
    let mut fixture = Fixture::new();
    std::fs::write(fixture.host.source.join("large.bin"), vec![0; 1024]).unwrap();
    Arc::get_mut(&mut fixture.host).unwrap().policy = WorkspacePolicy {
        workspace_bytes: 2048,
        total_bytes: 1600,
        ttl_secs: 3600,
    };
    let controller = fixture.kernel(provider(false, false, false));
    let mut first = fixture
        .host
        .prepare(&controller, "first", None)
        .await
        .unwrap();
    assert!(
        fixture
            .host
            .prepare(&controller, "no room", None)
            .await
            .is_err()
    );
    first
        .checkpoint
        .finish(&mut receipt(false, "failed but recorded"))
        .unwrap();
    assert!(!first.run.cwd.exists());
    let next = fixture
        .host
        .prepare(&controller, "room reclaimed", None)
        .await
        .unwrap();
    assert!(next.run.cwd.exists());
    drop(next);
    drop(first);
    drop(controller);
    Arc::get_mut(&mut fixture.host)
        .unwrap()
        .policy
        .workspace_bytes = 10;
    let controller = fixture.kernel(provider(false, false, false));
    assert!(
        fixture
            .host
            .prepare(&controller, "too large", None)
            .await
            .is_err()
    );
}

struct QuotaProvider {
    inner: Arc<Provider>,
}
#[async_trait]
impl ModelProvider for QuotaProvider {
    fn name(&self) -> &'static str {
        "quota-test"
    }
    fn model_id(&self) -> &'static str {
        "quota-test"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        if request
            .messages
            .iter()
            .any(|m| m.role == model::Role::User && m.content == "child 1")
        {
            return Ok(call(
                "oversize",
                "filesystem",
                json!({"operation":"write","path":"large.bin","content":"x".repeat(4096)}),
            ));
        }
        self.inner.complete(request).await
    }
}
#[tokio::test]
async fn tool_growth_quota_fails_only_that_child_then_reclaims_disk() {
    let mut fixture = Fixture::new();
    Arc::get_mut(&mut fixture.host)
        .unwrap()
        .policy
        .workspace_bytes = 1024;
    let mut kernel = fixture.kernel(Arc::new(QuotaProvider {
        inner: provider(false, false, false),
    }));
    kernel.run_turn(input(2), |_| {}).await.unwrap();
    let tasks = &kernel.task_queue().unwrap().tasks;
    assert_eq!(tasks[0].status, TaskStatus::Failed);
    assert!(tasks[0].failure_reason.as_ref().unwrap().contains("quota"));
    assert_eq!(tasks[1].status, TaskStatus::Completed);
    for task in tasks {
        assert!(!task.child.as_ref().unwrap().cwd.exists());
    }
}

#[tokio::test]
async fn legacy_workspace_store_migrates_before_resume_and_terminal_cleanup() {
    let fixture = Fixture::new();
    let controller = fixture.kernel(provider(false, false, false));
    let mut child = fixture
        .host
        .prepare(&controller, "child 1", None)
        .await
        .unwrap();
    child.checkpoint.save(&[Message::user("child 1")]).unwrap();
    let mut legacy = child.run.clone();
    let state = legacy.state_dir.take().unwrap();
    drop(child);
    let old = legacy.cwd.join(".ax");
    std::fs::create_dir_all(&old).unwrap();
    for entry in std::fs::read_dir(&state).unwrap() {
        let entry = entry.unwrap();
        if matches!(
            entry.file_name().to_str(),
            Some("workspace.lock" | "workspace.json")
        ) {
            continue;
        }
        std::fs::rename(entry.path(), old.join(entry.file_name())).unwrap();
    }
    let model = provider(false, false, false);
    let controller = fixture.kernel(model.clone());
    let mut recovered = fixture
        .host
        .prepare(&controller, "child 1", Some(&legacy))
        .await
        .unwrap();
    assert_eq!(recovered.run.state_dir.as_ref(), Some(&state));
    let mut checkpoint = recovered.checkpoint;
    let output = AgentSupervisor::run_child(
        &mut recovered.kernel,
        "child 1",
        Box::new(|_| {}),
        Box::new(|messages| checkpoint.save(messages)),
    )
    .await
    .unwrap();
    checkpoint.finish(&mut receipt(true, &output)).unwrap();
    assert!(!legacy.cwd.exists());
    let store = MemoryStore::open(state.join("child.sqlite3")).unwrap();
    let messages = store
        .load_messages(&legacy.session_id, None, u32::MAX)
        .unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.role == memory::MessageRole::User)
            .count(),
        1
    );
    assert!(
        store
            .latest_agent_state(&legacy.session_id, OUTCOME_PREFIX)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn gc_rejects_manifest_paths_outside_child_directory() {
    let fixture = Fixture::new();
    let controller = fixture.kernel(provider(false, false, false));
    let child = fixture
        .host
        .prepare(&controller, "owned", None)
        .await
        .unwrap();
    let state = child.run.state_dir.clone().unwrap();
    drop(child);
    let mut manifest = workspace::read_manifest(&state).unwrap();
    manifest.cwd = fixture.host.source.clone();
    manifest.status = "failed".into();
    workspace::write_manifest(&state, &manifest).unwrap();
    assert!(workspace::cleanup(&state).is_err());
    workspace::gc(&fixture.host.root, fixture.host.policy).unwrap();
    assert!(fixture.host.source.join("project.txt").exists());
    let next = fixture
        .host
        .prepare(&controller, "independent child", None)
        .await
        .unwrap();
    assert!(
        next.run.cwd.exists(),
        "deferred GC must not block independent children"
    );
}

#[tokio::test]
async fn resume_over_quota_child_persists_failure_before_cleanup() {
    let mut fixture = Fixture::new();
    Arc::get_mut(&mut fixture.host)
        .unwrap()
        .policy
        .workspace_bytes = 1024;
    let controller = fixture.kernel(provider(false, false, false));
    let child = fixture
        .host
        .prepare(&controller, "interrupted", None)
        .await
        .unwrap();
    let run = child.run.clone();
    std::fs::write(run.cwd.join("large.bin"), vec![0; 4096]).unwrap();
    drop(child);
    let recovered = fixture
        .host
        .prepare(&controller, "interrupted", Some(&run))
        .await
        .unwrap();
    let outcome = recovered.terminal.unwrap();
    assert!(!outcome.status.success());
    assert!(outcome.summary.contains("quota"));
    assert!(!run.cwd.exists());
    let store = MemoryStore::open(run.state_dir.unwrap().join("child.sqlite3")).unwrap();
    assert!(
        store
            .latest_agent_state(&run.session_id, OUTCOME_PREFIX)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn shared_child_workspace_keeps_parent_outputs_and_cleanup_is_owned() {
    let fixture = Fixture::new();
    let provider = provider(false, false, false);
    let controller = fixture.kernel(provider);
    let policy = runtime_core::ChildPolicy {
        workspace: runtime_core::child_policy::WorkspaceInheritance::Shared,
        ..Default::default()
    };
    let mut child = fixture
        .host
        .prepare_with_policy(&controller, "child 1", &policy)
        .await
        .unwrap();
    let disposable = child.run.cwd.clone();
    let output = child.kernel.run_turn("child 1", |_| {}).await.unwrap();
    assert!(fixture.host.source.join("result.txt").is_file());
    child
        .checkpoint
        .finish(&mut receipt(true, &output))
        .unwrap();
    assert!(!disposable.exists());
    assert!(fixture.host.source.join("result.txt").is_file());
}
