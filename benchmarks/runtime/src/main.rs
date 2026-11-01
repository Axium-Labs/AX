// Custom Cargo benchmark harness, calling public production APIs.
use async_trait::async_trait;
use memory::{MemoryRecord, MemoryScope, MemoryStore, MessageRole, NewMessage};
use model::{Message, ModelProvider, ModelRequest, ModelResponse};
use runtime_core::*;
use serde_json::{Value, json};
use std::{hint::black_box, sync::Arc, time::Instant};
use tool::{Capability, SafetyLevel, Tool, ToolError, ToolRegistry};

const TEXT: &str = include_str!("../datasets/message.txt");
static TOOL_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct MockProvider {
    calls: usize,
}
#[async_trait]
impl ModelProvider for MockProvider {
    fn name(&self) -> &str {
        "offline-bench"
    }
    fn model_id(&self) -> &str {
        "deterministic-v1"
    }
    fn context_window(&self) -> usize {
        1_000_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, model::ModelError> {
        tokio::task::yield_now().await;
        let done = request.messages.iter().any(|m| m.role == model::Role::Tool);
        Ok(if self.calls > 0 && !done {
            ModelResponse {
                tool_calls: (0..self.calls)
                    .map(|i| model::ToolCall {
                        id: format!("call-{i}"),
                        kind: "function".into(),
                        function: model::FunctionCall {
                            name: "bench0".into(),
                            arguments: "{}".into(),
                        },
                    })
                    .collect(),
                ..Default::default()
            }
        } else {
            ModelResponse {
                content: "bench-complete".into(),
                ..Default::default()
            }
        })
    }
}
struct MockTool(String);
#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.0
    }
    fn description(&self) -> &str {
        "deterministic offline boundary"
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object"})
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::Safe
    }
    fn capability(&self, _: &Value) -> Capability {
        Capability::FilesystemRead
    }
    fn resources(&self, _: &Value) -> Vec<tool::ResourceAccess> {
        vec![]
    }
    async fn execute(&self, _: Value) -> Result<String, ToolError> {
        TOOL_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok("mock-result".into())
    }
}
struct MockHost;
struct MockCheckpoint;
impl ChildCheckpoint for MockCheckpoint {
    fn save(&mut self, _: &[Message]) -> Result<(), AgentError> {
        Ok(())
    }
    fn finish(&mut self, _: &mut ChildResult) -> Result<(), AgentError> {
        Ok(())
    }
}
#[async_trait]
impl ChildHost for MockHost {
    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        _: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError> {
        let run = ChildRun {
            goal_id: "bench-goal".into(),
            session_id: "bench-child".into(),
            cwd: std::env::current_dir().unwrap(),
            workspace_root: None,
            memory_scope: "bench-child".into(),
            state_dir: None,
            execution_budget: None,
        };
        Ok(PreparedChild {
            kernel: controller.fork_child(run.clone(), input, vec![]),
            run,
            checkpoint: Box::new(MockCheckpoint),
            terminal: None,
        })
    }
}
fn kernel(calls: usize) -> AgentKernel {
    let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
    registry.register(MockTool("bench0".into()));
    AgentKernel::new(
        Arc::new(MockProvider { calls }),
        registry,
        Arc::new(AllowAll),
    )
    .with_execution_scope(std::env::current_dir().unwrap())
    .with_execution_budget(ExecutionBudget {
        max_steps: 8,
        ..Default::default()
    })
}
#[allow(
    clippy::assertions_on_constants,
    reason = "reject accidental debug measurements at runtime"
)]
fn main() {
    assert!(!cfg!(debug_assertions), "benchmarks require release");
    assert_eq!(
        std::env::var_os("AX_BENCH_WORKDIR").map(std::path::PathBuf::from),
        Some(std::env::current_dir().unwrap()),
        "run through run.py with an isolated working directory"
    );
    assert!(
        !std::path::Path::new("fixture.db").exists(),
        "fixture must be fresh"
    );
    let args: Vec<_> = std::env::args().collect();
    // Cargo may append --bench; explicit parameters precede it.
    let case = &args[1];
    let size: usize = args[2].parse().unwrap();
    let samples: usize = args[3].parse().unwrap();
    let warmup: usize = args[4].parse().unwrap();
    let concurrency: usize = args[5].parse().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let messages: Vec<_> = (0..size)
        .map(|i| {
            if i % 2 == 0 {
                Message::user(TEXT)
            } else {
                Message::assistant(TEXT, vec![])
            }
        })
        .collect();
    let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
    for i in 0..size {
        registry.register(MockTool(format!("bench{i}")));
    }
    let mut store = MemoryStore::open("fixture.db").unwrap();
    let session = store.create_session("fixed benchmark session").unwrap();
    let stored: Vec<_> = (0..size)
        .map(|i| {
            NewMessage::text(
                if i % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                },
                TEXT,
            )
        })
        .collect();
    // Seed only the datasets relevant to the case; setup is outside latency timing.
    if case.starts_with("session_") && case != "session_create" {
        store.append_batch(&session.id, &stored).unwrap();
    }
    if case == "session_snapshot_load" {
        store
            .save_effective_context(
                &session.id,
                "fixed local snapshot",
                &serde_json::to_string(&messages).unwrap(),
            )
            .unwrap();
    }
    if case.starts_with("memory_") {
        for i in 0..size {
            store.remember_scoped(&record(i)).unwrap();
        }
    }
    let mut rows = vec![];
    for sample in 0..warmup + samples {
        let repetitions = if case.starts_with("context_") || case == "tool_lookup" {
            100
        } else {
            1
        };
        let mut elapsed = 0_u128;
        let mut operations = 0;
        for _ in 0..repetitions {
            // Kernel construction is independently measured; turn/dispatch cases exclude it.
            let mut k = kernel(if case == "tool_dispatch" { size } else { 0 });
            let supervisor = AgentSupervisor::new(kernel(0), concurrency);
            let tasks = (0..size)
                .map(|i| AgentTask {
                    id: format!("task-{i}"),
                    prompt: TEXT.into(),
                    context: vec![],
                })
                .collect();
            let mut created = None;
            TOOL_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
            let start = Instant::now();
            match case.as_str() {
                "kernel_init" => {
                    black_box(kernel(0));
                }
                "session_create" => {
                    created = Some(store.create_session("fixed").unwrap().id);
                }
                "session_load" => {
                    assert!(store.session(&session.id).unwrap().is_some());
                    assert_eq!(
                        store
                            .load_messages(&session.id, None, size as u32)
                            .unwrap()
                            .len(),
                        size
                    );
                }
                "session_resume_local" => {
                    black_box(store.effective_context(&session.id).unwrap());
                    assert_eq!(
                        store
                            .load_context_page(&session.id, None, 128)
                            .unwrap()
                            .len(),
                        size.min(128)
                    );
                    black_box(select_context(&messages, 8192, 1024));
                }
                "session_snapshot_load" => {
                    let json = store.effective_context(&session.id).unwrap().unwrap();
                    let restored: Vec<Message> = serde_json::from_str(&json).unwrap();
                    assert_eq!(restored.len(), size);
                    assert!(
                        store
                            .load_context_page(&session.id, None, 128)
                            .unwrap()
                            .is_empty()
                    );
                    black_box(select_context(&restored, 8192, 1024));
                }
                "context_select" => {
                    black_box(select_context(black_box(&messages), 8192, 1024));
                }
                "context_serialize" => {
                    black_box(serde_json::to_vec(black_box(&messages)).unwrap());
                }
                "context_tokens" => {
                    black_box(estimate_tokens(black_box(&messages)));
                }
                "memory_query" => {
                    assert_eq!(
                        store
                            .memory_index(MemoryScope::Project, "bench-project")
                            .unwrap()
                            .len(),
                        size
                    );
                }
                "memory_recall" => {
                    let entries = store
                        .memory_index(MemoryScope::Project, "bench-project")
                        .unwrap();
                    let recalled =
                        memory::retrieve_index(entries, "build command runtime", 1_800_000_000);
                    assert!(!recalled.is_empty());
                    black_box(recalled);
                }
                "memory_store" => {
                    store.remember_scoped(&record(0)).unwrap();
                }
                "tool_register" => {
                    let mut r = ToolRegistry::with_mode(tool::SandboxMode::Off);
                    for i in 0..size {
                        r.register(MockTool(format!("bench{i}")));
                    }
                    black_box(r);
                }
                "tool_lookup" => {
                    black_box(registry.get(black_box("bench0")).unwrap());
                }
                "mock_tool" => {
                    black_box(
                        rt.block_on(MockTool("bench0".into()).execute(json!({})))
                            .unwrap(),
                    );
                }
                "mock_model" => {
                    black_box(
                        rt.block_on(MockProvider { calls: 0 }.complete(ModelRequest {
                            messages: vec![Message::user(TEXT)],
                            tools: vec![],
                        }))
                        .unwrap(),
                    );
                }
                "tool_dispatch" | "kernel_turn" => {
                    let result = rt.block_on(k.run_turn(TEXT, |_| {})).unwrap();
                    assert_eq!(result, "bench-complete");
                }
                "subagent_lifecycle" => {
                    k = k.with_child_host(Arc::new(MockHost));
                    rt.block_on(async {
                        let _events = k.prepare_subagents().unwrap();
                        let id = k.spawn_agent(TEXT, SpawnOptions::default()).unwrap();
                        let result = k.wait_agent(&id).await;
                        assert_eq!(result.status, "completed", "{:?}", result);
                    });
                }
                "concurrency" => {
                    let results = rt.block_on(supervisor.run_tasks(tasks, None)).unwrap();
                    assert_eq!(results.len(), size);
                    for r in results {
                        assert_eq!(r.result.unwrap(), "bench-complete");
                    }
                }
                _ => panic!("unknown case: {case}"),
            }
            elapsed += start.elapsed().as_nanos();
            if case == "tool_dispatch" {
                assert_eq!(
                    TOOL_CALLS.load(std::sync::atomic::Ordering::Relaxed),
                    size,
                    "dispatch must execute every mock tool"
                );
            }
            operations += if matches!(
                case.as_str(),
                "tool_register" | "tool_dispatch" | "concurrency"
            ) {
                size
            } else {
                1
            };
            if let Some(id) = created {
                store.delete_session(&id).unwrap();
            }
        }
        if sample >= warmup {
            rows.push(json!({"sample":sample-warmup,"elapsed_ns":elapsed,"operations":operations,"ns_per_op":elapsed as f64 / operations as f64}));
        }
    }
    println!(
        "{}",
        json!({"case":case,"size":size,"concurrency":concurrency,"samples":rows})
    );
}
fn record(i: usize) -> MemoryRecord {
    MemoryRecord {
        key: format!("build.command.{i:06}"),
        value: format!("cargo build runtime module {i}. {TEXT}"),
        scope: MemoryScope::Project,
        owner: "bench-project".into(),
        source: "fixed-fixture-v1".into(),
        updated_at: 1_800_000_000,
        ..Default::default()
    }
}
