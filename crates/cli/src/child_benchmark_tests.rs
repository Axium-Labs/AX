//! Child startup component benchmark.
//!
//! Not part of the normal suite: it measures, it does not assert. Run it
//! explicitly:
//!
//! ```bash
//! cargo test -p cli --bin ax child_benchmark -- --ignored --nocapture --test-threads=1
//! ```

// Benchmark magnitudes sit far below f64 precision limits; the `as f64`
// conversions are deliberate.
#![allow(clippy::cast_precision_loss, clippy::cast_lossless)]
//!
//! The four numbers the child startup critical-path work is judged on:
//! `workspace_create_ms`, `db_checkpoint_ms`, `quota_scan_ms`, `child_startup_ms`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall};
use runtime_core::{AgentKernel, AllowAll, ExecutionBudget};
use serde_json::json;
use tool::ToolRegistry;

use crate::child_runtime::LocalChildHost;

struct Fixture {
    root: PathBuf,
    host: Arc<LocalChildHost>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ax-child-bench-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("project.txt"), "controller baseline").unwrap();
        let host = Arc::new(LocalChildHost {
            sandbox: std::sync::OnceLock::new(),
            policy: crate::child_runtime::WorkspacePolicy::default(),
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

fn text(content: &str) -> ModelResponse {
    ModelResponse {
        provider_metadata: None,
        content: content.into(),
        tool_calls: vec![],
        usage: None,
        finish_reason: None,
    }
}

#[allow(clippy::needless_pass_by_value)] // fixture: literals only, readability over moves
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

struct Provider {
    requests: Mutex<Vec<ModelRequest>>,
}

#[async_trait]
impl ModelProvider for Provider {
    fn name(&self) -> &'static str {
        "bench"
    }
    fn model_id(&self) -> &'static str {
        "bench"
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
                let prompt = request
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.role == model::Role::User)
                    .map_or("", |m| m.content.as_str());
                let tasks = runtime_core::task_queue::TaskQueue::list_hints(prompt);
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
        let input = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == model::Role::User)
            .map_or("", |m| m.content.as_str());
        let phase = request
            .messages
            .iter()
            .filter(|m| m.role == model::Role::Tool)
            .count();
        Ok(match phase {
            0 => call(
                "write",
                "filesystem",
                json!({"operation":"write","path":"result.txt","content":input}),
            ),
            1 => call(
                "remember",
                "memory",
                json!({"action":"set","scope":"session","key":"task","value":input}),
            ),
            _ => text(&format!("outcome:{input}")),
        })
    }
}

fn mean(before: &tool::telemetry::Metric, after: &tool::telemetry::Metric) -> f64 {
    let count = after.count - before.count;
    if count == 0 {
        return 0.0;
    }
    (after.total_micros - before.total_micros) as f64 / count as f64 / 1_000.0
}

fn report(before: &tool::telemetry::Metric, after: &tool::telemetry::Metric, label: &str) {
    let count = after.count - before.count;
    let mean = mean(before, after);
    eprintln!("{label}_ms={mean:.3} (count={count})");
}

#[tokio::test]
#[ignore = "benchmark; run explicitly"]
async fn child_startup_component_benchmark() {
    let count = 23;
    let fixture = Fixture::new();
    let provider = Arc::new(Provider {
        requests: Mutex::new(vec![]),
    });
    let mut kernel = fixture
        .kernel(provider)
        .with_child_concurrency(4)
        .with_execution_budget(ExecutionBudget::default());
    let before = tool::telemetry::snapshot();
    let started = std::time::Instant::now();
    kernel.run_turn(input(count), |_| {}).await.unwrap();
    let wall = started.elapsed();
    let after = tool::telemetry::snapshot();
    let get = |map: &std::collections::BTreeMap<String, tool::telemetry::Metric>, name: &str| {
        map.get(name).cloned().unwrap_or_default()
    };
    report(
        &get(&before, "child.workspace_create"),
        &get(&after, "child.workspace_create"),
        "workspace_create",
    );
    report(
        &get(&before, "child.db_checkpoint"),
        &get(&after, "child.db_checkpoint"),
        "db_checkpoint",
    );
    report(
        &get(&before, "child.quota_scan"),
        &get(&after, "child.quota_scan"),
        "quota_scan",
    );
    report(
        &get(&before, "child.startup"),
        &get(&after, "child.startup"),
        "child_startup",
    );
    eprintln!(
        "child_wall_ms={:.3} (total={:.0}ms / {count} children)",
        wall.as_secs_f64() * 1_000.0 / count as f64,
        wall.as_millis(),
    );
}

/// Isolate the session-database initialization cost: open a fresh child store
/// and create its session, N times.
#[tokio::test]
#[ignore = "benchmark; run explicitly"]
async fn session_database_init_benchmark() {
    let root = std::env::temp_dir().join(format!("ax-bench-db-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut total = std::time::Duration::ZERO;
    let count = 23;
    for index in 0..count {
        let database = root.join(format!("db-{index}.sqlite3"));
        let started = std::time::Instant::now();
        let store = memory::MemoryStore::open(&database).unwrap();
        let session = store.create_session("bench").unwrap();
        let elapsed = started.elapsed();
        total += elapsed;
        eprintln!(
            "  session_db_init[{index}]_ms={:.3}",
            elapsed.as_secs_f64() * 1_000.0
        );
        drop(store);
        let _ = session;
    }
    eprintln!(
        "session_db_init_ms={:.3} (count={count})",
        total.as_secs_f64() * 1_000.0 / count as f64
    );
    let _ = std::fs::remove_dir_all(&root);
}
