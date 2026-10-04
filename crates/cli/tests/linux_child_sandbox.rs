#[cfg(not(target_os = "linux"))]
fn main() {}
#[cfg(target_os = "linux")]
#[path = "../src/child_runtime.rs"]
#[allow(
    dead_code,
    unused_imports,
    reason = "This harness reuses the production host and only a subset of its unit-test helpers"
)]
mod child_runtime;
#[cfg(target_os = "linux")]
#[path = "../src/memory_tool.rs"]
#[allow(
    dead_code,
    unused_imports,
    reason = "Only inherited memory APIs are used by this integration harness"
)]
mod memory_tool;
#[cfg(target_os = "linux")]
#[path = "../src/worktree_changes.rs"]
#[allow(
    dead_code,
    reason = "Only the snapshot worker is used by this integration harness"
)]
mod worktree_changes;
#[cfg(target_os = "linux")]
fn memory_role(role: &model::Role) -> memory::MessageRole {
    match role {
        model::Role::User => memory::MessageRole::User,
        model::Role::Assistant => memory::MessageRole::Assistant,
        model::Role::System => memory::MessageRole::System,
        model::Role::Tool => memory::MessageRole::Tool,
    }
}
#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--ax-sandbox-broker") => {
            sandbox::run_broker(args.next().unwrap().into())
                .await
                .unwrap();
            return;
        }
        Some("--ax-sandbox-proxy") => {
            let socket = args.next().unwrap();
            let spec = args.next().unwrap();
            std::process::exit(sandbox::run_proxy(socket.into(), &spec).await.unwrap());
        }
        Some("--ax-sandbox-worker") => {
            tool::sandbox_worker(&args.next().unwrap()).await.unwrap();
            return;
        }
        Some("--ax-sandbox-snapshot") => {
            worktree_changes::worker_snapshot().unwrap();
            return;
        }
        _ => {}
    }
    run().await;
}
#[cfg(target_os = "linux")]
#[allow(
    clippy::too_many_lines,
    reason = "One end-to-end child provisioning and cleanup scenario"
)]
async fn run() {
    use runtime_core::{AgentKernel, AllowAll, ChildHost};
    use serde_json::json;
    use std::sync::{Arc, OnceLock};
    struct Provider;
    #[async_trait::async_trait]
    impl model::ModelProvider for Provider {
        fn name(&self) -> &'static str {
            "sandbox-test"
        }
        fn model_id(&self) -> &'static str {
            "sandbox-test"
        }
        fn context_window(&self) -> usize {
            100_000
        }
        async fn complete(
            &self,
            request: model::ModelRequest,
        ) -> Result<model::ModelResponse, model::ModelError> {
            if let Some(message) = request
                .messages
                .iter()
                .find(|m| m.tool_call_id.as_deref() == Some("development"))
            {
                assert!(
                    message.content.contains("SANDBOX_OK"),
                    "{}",
                    message.content
                );
                return Ok(model::ModelResponse {
                    provider_metadata: None,
                    content: "completed".into(),
                    tool_calls: vec![],
                    usage: None,
                    finish_reason: None,
                });
            }
            Ok(model::ModelResponse { provider_metadata: None, content:String::new(), usage:None, finish_reason:None,
                tool_calls:vec![model::ToolCall { id:"development".into(), kind:"function".into(), function:model::FunctionCall { name:"shell".into(), arguments:json!({"command":"set -e; test -d .git; git status --porcelain; git -c user.name=AX -c user.email=ax@example.invalid commit --allow-empty -qm child; printf done > child-output; if cat ../state/child.sqlite3; then exit 99; fi; echo SANDBOX_OK"}).to_string() } }] })
        }
    }
    let fixture = std::env::temp_dir().join(format!("ax-native-child-{}", uuid::Uuid::new_v4()));
    let source = fixture.join("source");
    let storage = fixture.join("ax-private");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&storage).unwrap();
    std::fs::write(storage.join("auth.json"), "host credential").unwrap();
    std::fs::write(source.join("project.txt"), "baseline\n").unwrap();
    let setup = std::process::Command::new("git")
        .arg("-C")
        .arg(&source)
        .args(["init", "-q"])
        .status()
        .unwrap();
    assert!(setup.success());
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["add", "project.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&source)
            .args([
                "-c",
                "user.name=AX",
                "-c",
                "user.email=ax@example.invalid",
                "commit",
                "-qm",
                "baseline"
            ])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(source.join("project.txt"), "dirty controller input\n").unwrap();
    std::env::set_current_dir(&source).unwrap();
    sandbox::SandboxManager::configure(sandbox::SandboxMode::Strict, vec![storage.clone()])
        .unwrap();
    let mut tools = tool::ToolRegistry::new();
    tools.register(tool::ShellTool);
    tools.register(tool::FilesystemTool);
    let controller = AgentKernel::new(Arc::new(Provider), tools, Arc::new(AllowAll));
    let host = child_runtime::LocalChildHost {
        source: source.clone(),
        root: storage.join("child-runs"),
        excluded: vec![storage.clone()],
        policy: child_runtime::WorkspacePolicy::default(),
        sandbox: OnceLock::new(),
    };
    let mut child = host
        .prepare(&controller, "isolated development", None)
        .await
        .unwrap();
    assert!(
        child.run.cwd.join(".git").is_dir(),
        "child must own its Git metadata"
    );
    assert_eq!(
        std::fs::read_to_string(child.run.cwd.join("project.txt")).unwrap(),
        "dirty controller input\n"
    );
    child
        .kernel
        .run_turn("isolated development", |_| {})
        .await
        .unwrap();
    assert!(child.run.cwd.join("child-output").is_file());
    child
        .checkpoint
        .finish(&runtime_core::ChildOutcome {
            success: true,
            output: "completed".into(),
        })
        .unwrap();
    assert!(!child.run.cwd.exists(), "private clone cleanup failed");
    assert_eq!(
        std::fs::read_to_string(storage.join("auth.json")).unwrap(),
        "host credential"
    );
    assert!(!source.join("child-output").exists());
    drop(child);
    drop(host);
    drop(controller);
    std::env::set_current_dir(std::env::temp_dir()).unwrap();
    std::fs::remove_dir_all(fixture).unwrap();
    println!(
        "Real ChildHost provisioning, private Git clone, dirty patch, child tool confinement and lifecycle cleanup passed"
    );
}
