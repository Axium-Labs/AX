//! Exercise the prompt adapters used by the three frontends, with real local children.
use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall};
use runtime_core::{
    AgentKernel, AllowAll, ApprovalPolicy, ExecutionBudget, task_queue::TaskStatus,
};
use serde_json::{Value, json};
use tool::{FilesystemTool, ToolRegistry};

use crate::{
    acp,
    commands::run::{run_prompt, run_prompt_with},
    model_selection::{ModelSelection, ProviderKind},
    repl::ReplState,
};

struct DataProvider {
    requests: Mutex<Vec<ModelRequest>>,
    dataset: PathBuf,
    fail_first: bool,
    requirements_only: bool,
}
fn text(content: &str) -> ModelResponse {
    ModelResponse {
        content: content.into(),
        tool_calls: vec![],
        usage: None,
        finish_reason: None,
    }
}
fn call(id: &str, name: &str, input: &Value) -> ModelResponse {
    ModelResponse {
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
#[async_trait]
impl ModelProvider for DataProvider {
    fn name(&self) -> &'static str {
        "prompt-child-test"
    }
    fn model_id(&self) -> &'static str {
        "prompt-child-test"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        if request
            .messages
            .iter()
            .any(|message| message.content.starts_with("[ax-completion-review]"))
        {
            return Ok(call(
                "review",
                "completion_check",
                &json!({"state":"complete","reason":"scripted fixture deliverables complete"}),
            ));
        }
        self.requests.lock().unwrap().push(request.clone());
        if self.requirements_only {
            assert!(
                !request
                    .messages
                    .iter()
                    .any(|m| m.content.starts_with("[ax-progress]"))
            );
            return Ok(text("requirements understood"));
        }
        if request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-child-runtime]"))
        {
            let users = request
                .messages
                .iter()
                .filter(|m| m.role == model::Role::User)
                .collect::<Vec<_>>();
            assert_eq!(users.len(), 1);
            let input = &users[0].content;
            assert!(input.contains("from instances.json. Produce a terminal outcome."));
            assert!(
                !request
                    .messages
                    .iter()
                    .any(|m| m.content.starts_with("[ax-progress]"))
            );
            if self.fail_first && input.starts_with("Process instance 1 ") {
                return Err(ModelError::Configuration("instance 1 unavailable".into()));
            }
            if !request.messages.iter().any(|m| m.role == model::Role::Tool) {
                return Ok(call(
                    "child-read",
                    "filesystem",
                    &json!({"operation":"read","path":"instances.json"}),
                ));
            }
            return Ok(text(&format!("completed: {input}")));
        }
        if request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-task-summary]"))
        {
            return Ok(text("all instance outcomes collected"));
        }
        let read = request
            .messages
            .iter()
            .find(|m| m.tool_call_id.as_deref() == Some("dataset"));
        let Some(read) = read else {
            assert!(
                !request
                    .messages
                    .iter()
                    .any(|m| m.content.starts_with("[ax-progress]"))
            );
            return Ok(call(
                "dataset",
                "filesystem",
                &json!({"operation":"read","path":self.dataset}),
            ));
        };
        let result: Value = serde_json::from_str(&read.content).unwrap();
        assert_eq!(result["status"], "success");
        let instances: Vec<usize> =
            serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
        let tasks = instances.iter().map(|id| json!({
            "title":format!("instance {id}"),
            "input":format!("Process instance {id} from instances.json. Produce a terminal outcome."),
        })).collect::<Vec<_>>();
        Ok(call(
            "plan",
            "task_queue",
            &json!({"action":"start","execution":"children","overall_goal":"process dataset instances","tasks":tasks}),
        ))
    }
}
#[derive(Clone, Copy)]
enum Surface {
    Cli,
    Tui,
    Acp,
}
struct Fixture {
    root: PathBuf,
    state: ReplState,
    provider: Arc<DataProvider>,
    selection: ModelSelection,
}
impl Fixture {
    fn new(count: usize, fail_first: bool, requirements_only: bool) -> Self {
        let root = std::env::temp_dir().join(format!("ax-prompt-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let dataset = project.join("instances.json");
        fs::write(
            &dataset,
            serde_json::to_string(&(1..=count).collect::<Vec<_>>()).unwrap(),
        )
        .unwrap();
        let provider = Arc::new(DataProvider {
            requests: Mutex::new(vec![]),
            dataset,
            fail_first,
            requirements_only,
        });
        let mut state =
            ReplState::new_in_project(root.join("data"), root.join("skills"), None, &project)
                .unwrap();
        state.create_session("prompt execution regression").unwrap();
        state.allowed_skills = Some(HashSet::new());
        state.execution_budget = ExecutionBudget {
            max_steps: 100,
            max_tool_calls: 100,
            turn_timeout_secs: 60,
            tool_timeout_secs: 5,
        };
        state.child_timeout_secs = 7;
        let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
        tools.register(FilesystemTool);
        // Deliberately start without a ChildHost or child budget: production wiring must add both.
        state.runtime = Some(AgentKernel::new(
            provider.clone(),
            tools,
            Arc::new(AllowAll),
        ));
        let selection = ModelSelection {
            provider: ProviderKind::Compatible,
            provider_id: "orchestration-test-no-network".into(),
            endpoint: None,
            model: "test".into(),
            codex_auth: None,
            context_window: Some(100_000),
            max_output_tokens: Some(1000),
            reasoning_effort: None,
            supports_tools: true,
        };
        Self {
            root,
            state,
            provider,
            selection,
        }
    }
    async fn run(&mut self, surface: Surface, prompt: &str) -> Vec<Value> {
        let approval: Arc<dyn ApprovalPolicy> = Arc::new(AllowAll);
        let mut updates = vec![];
        let output = match surface {
            Surface::Cli => run_prompt(&mut self.state, &self.selection, approval, prompt).await,
            Surface::Tui => {
                run_prompt_with(&mut self.state, &self.selection, approval, prompt, |_| {}).await
            }
            Surface::Acp => {
                self.state.ensure_session(prompt).unwrap();
                let session = self.state.current_session_id().unwrap().to_owned();
                let (out, mut incoming) = tokio::sync::mpsc::unbounded_channel();
                let result = acp::run_session_prompt(
                    &mut self.state,
                    &self.selection,
                    approval,
                    prompt,
                    &out,
                    &session,
                )
                .await;
                while let Ok(update) = incoming.try_recv() {
                    updates.push(update);
                }
                result
            }
        }
        .unwrap();
        assert_eq!(
            output,
            if self.provider.requirements_only {
                "requirements understood"
            } else {
                "all instance outcomes collected"
            }
        );
        self.state.evolution_finish().await;
        updates
    }
    fn check_children(&self, count: usize) {
        let kernel = self.state.runtime.as_ref().unwrap();
        assert_eq!(kernel.child_execution_budget().turn_timeout_secs, 7);
        assert_eq!(kernel.child_execution_budget().max_tool_calls, 100);
        let queue = kernel.task_queue().unwrap();
        assert_eq!(queue.tasks.len(), count);
        assert_eq!(queue.state, runtime_core::QueueState::Completed);
        let mut identities = HashSet::new();
        for (index, task) in queue.tasks.iter().enumerate() {
            assert_eq!(task.title, format!("instance {}", index + 1));
            assert_eq!(
                task.input,
                format!(
                    "Process instance {} from instances.json. Produce a terminal outcome.",
                    index + 1
                )
            );
            assert_eq!(
                task.status,
                if self.provider.fail_first && index == 0 {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Completed
                }
            );
            let child = task.child.as_ref().unwrap();
            assert!(identities.insert(child.session_id.clone()));
            assert_eq!(child.execution_budget.unwrap().turn_timeout_secs, 7);
            assert!(
                child
                    .state_dir
                    .as_ref()
                    .unwrap()
                    .join("child.sqlite3")
                    .exists()
            );
            assert!(!child.cwd.exists());
        }
        let requests = self.provider.requests.lock().unwrap();
        let mut starts = requests
            .iter()
            .filter(|r| {
                r.messages
                    .iter()
                    .any(|m| m.content.starts_with("[ax-child-runtime]"))
                    && !r.messages.iter().any(|m| m.role == model::Role::Tool)
            })
            .map(|r| {
                r.messages
                    .iter()
                    .find(|m| m.role == model::Role::User)
                    .unwrap()
                    .content
                    .clone()
            })
            .collect::<Vec<_>>();
        let mut expected = queue
            .tasks
            .iter()
            .map(|task| task.input.clone())
            .collect::<Vec<_>>();
        starts.sort();
        expected.sort();
        assert_eq!(
            starts, expected,
            "all independent children execute without finish controls"
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| !r
                    .messages
                    .iter()
                    .any(|m| m.content.starts_with("[ax-child-runtime]")))
                .count(),
            if fs::read_to_string(&self.provider.dataset)
                .unwrap()
                .contains("ax_work_items")
            {
                2
            } else {
                3
            },
            "typed inventory skips the manual plan round"
        );
    }
    fn cleanup(self) {
        let root = self.root.clone();
        drop(self);
        fs::remove_dir_all(root).unwrap();
    }
}
#[tokio::test]
async fn cli_tui_acp_share_real_local_children_and_independent_child_budget() {
    for surface in [Surface::Cli, Surface::Tui, Surface::Acp] {
        let mut fixture = Fixture::new(3, false, false);
        let updates = fixture
            .run(
                surface,
                "Read instances.json and process its independent instances",
            )
            .await;
        fixture.check_children(3);
        if matches!(surface, Surface::Acp) {
            let tools = updates
                .iter()
                .filter(|u| u.pointer("/params/update/sessionUpdate") == Some(&json!("tool_call")))
                .collect::<Vec<_>>();
            assert_eq!(tools.len(), 4, "controller read plus three child reads");
            assert!(tools[1..].iter().all(|u| {
                u.pointer("/params/update/toolCallId")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .contains(':')
            }));
            let chunks = updates
                .iter()
                .filter_map(|u| {
                    u.pointer("/params/update/content/text")
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>();
            assert_eq!(chunks, vec!["all instance outcomes collected"]);
        }
        fixture.cleanup();
    }
}
#[tokio::test]
async fn acp_first_child_failure_does_not_stop_second_child() {
    let mut fixture = Fixture::new(3, true, false);
    fixture
        .run(
            Surface::Acp,
            "Read instances.json and process its independent instances",
        )
        .await;
    fixture.check_children(3);
    fixture.cleanup();
}
#[tokio::test]
async fn acp_reads_data_then_dynamically_creates_twenty_three_complete_child_inputs() {
    let mut fixture = Fixture::new(23, false, false);
    fixture
        .run(
            Surface::Acp,
            "Read instances.json; discover the instance count from the data",
        )
        .await;
    fixture.check_children(23);
    fixture.cleanup();
}
#[tokio::test]
async fn benchmark_numbered_rules_fields_and_bullets_never_create_executable_tasks() {
    let rules = (1..=30)
        .map(|i| format!("{i}. metric_{i}: informational benchmark rule"))
        .collect::<Vec<_>>()
        .join("\n");
    for surface in [Surface::Cli, Surface::Tui, Surface::Acp] {
        let mut fixture = Fixture::new(0, false, true);
        fixture.run(surface,&format!("Explain benchmark requirements\n{rules}\nFields:\n- instance_id\n- score\n- elapsed_time")).await;
        assert!(
            fixture
                .state
                .runtime
                .as_ref()
                .unwrap()
                .task_queue()
                .is_none_or(|queue| queue.tasks.is_empty())
        );
        assert!(!fixture.state.data_dir.join("child-runs").exists());
        assert_eq!(fixture.provider.requests.lock().unwrap().len(), 1);
        fixture.cleanup();
    }
}

#[path = "../../../../test/harness/frontend.rs"]
mod harness_frontend_tests;
