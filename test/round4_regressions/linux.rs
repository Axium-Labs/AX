#[cfg(not(target_os = "linux"))]
fn main() {
    println!("SKIP round4 Linux sandbox cases: target is not Linux (6 cases)");
}

#[cfg(target_os = "linux")]
mod suite {
    use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall};
    use sandbox::{CommandSpec, SandboxManager, SandboxMode};
    use serde_json::{Value, json};
    use std::{path::PathBuf, sync::Arc, time::Duration};
    use tool::Tool;

    struct Provider {
        name: &'static str,
        input: Value,
    }

    #[async_trait::async_trait]
    impl ModelProvider for Provider {
        fn name(&self) -> &'static str {
            "round4-regression"
        }
        fn model_id(&self) -> &'static str {
            "round4-regression"
        }
        fn context_window(&self) -> usize {
            32_000
        }
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            if request
                .messages
                .iter()
                .any(|message| message.role == model::Role::Tool)
            {
                return Ok(ModelResponse {
                    content: "The tool result was received.".into(),
                    ..Default::default()
                });
            }
            Ok(ModelResponse {
                tool_calls: vec![ToolCall {
                    id: "missing-path".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: self.name.into(),
                        arguments: self.input.to_string(),
                    },
                }],
                ..Default::default()
            })
        }
    }

    async fn missing_path_is_an_ordinary_error(root: PathBuf, name: &'static str, input: Value) {
        let inner: Arc<dyn Tool> = match name {
            "filesystem" => Arc::new(tool::FilesystemTool),
            "search" => Arc::new(tool::SearchTool::new(root.clone())),
            _ => unreachable!(),
        };
        let worker = Arc::new(tool::SandboxedTool::new(inner, root));
        let outcome = worker.execute_output(input.clone()).await;
        let mut tools = tool::ToolRegistry::new();
        tools.register_arc(worker);
        let mut kernel = runtime_core::AgentKernel::new(
            Arc::new(Provider { name, input }),
            tools,
            Arc::new(runtime_core::AllowAll),
        );
        let mut observed = Vec::new();
        let result = kernel
            .run_goal_turn(
                "Inspect the requested path",
                runtime_core::GoalTurn::Pursue {
                    config: runtime_core::GoalConfig {
                        mode: runtime_core::GoalMode::Deterministic,
                        max_rounds: 1,
                        ..Default::default()
                    },
                    goal_id: None,
                },
                |event| {
                    if let runtime_core::AgentEvent::ToolFinished { result, .. } = event {
                        observed.push(result);
                    }
                },
            )
            .await;
        assert_eq!(observed.len(), 1, "{result:?}");
        println!(
            "OBSERVED {name}: status={}, permission_denied={}, output={}",
            observed[0].status, observed[0].permission_denied, observed[0].raw_output
        );
        assert!(!observed[0].permission_denied);
        match outcome {
            Err(error) => assert!(
                !matches!(
                    error,
                    tool::ToolError::SandboxViolation(_) | tool::ToolError::PermissionDenied(_)
                ),
                "{error}"
            ),
            Ok(output) => panic!("expected ordinary missing-path error, got success: {output:?}"),
        }
        assert_eq!(observed[0].status, "error");
    }

    async fn outside_symlink_is_a_sandbox_violation(root: PathBuf) {
        let outside = root.parent().unwrap().join("outside-secret");
        std::fs::write(&outside, "private").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let worker = tool::SandboxedTool::new(Arc::new(tool::FilesystemTool), root);
        let error = worker
            .execute(json!({"operation":"read","path":"escape"}))
            .await
            .unwrap_err();
        assert!(
            matches!(error, tool::ToolError::SandboxViolation(_)),
            "{error}"
        );
    }

    async fn shell(manager: &SandboxManager, command: &str) -> std::process::Output {
        let mut spec = CommandSpec::new("/bin/sh");
        spec.args = vec!["-c".into(), command.into()];
        tokio::time::timeout(
            Duration::from_secs(15),
            manager.spawn(spec).unwrap().wait_with_output(),
        )
        .await
        .unwrap()
        .unwrap()
    }

    async fn successful_command_keeps_background_child(root: PathBuf) {
        let manager = SandboxManager::for_workspace(root).unwrap();
        let first = shell(&manager, "sleep 30 & echo $! > bg.pid").await;
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        let next = shell(&manager, "kill -0 $(cat bg.pid)").await;
        let _ = shell(&manager, "kill $(cat bg.pid)").await;
        assert!(
            next.status.success(),
            "{}",
            String::from_utf8_lossy(&next.stderr)
        );
    }

    async fn proxy_disconnect_kills_background_child(root: PathBuf) {
        let manager = SandboxManager::for_workspace(root.clone()).unwrap();
        let mut spec = CommandSpec::new("/bin/sh");
        spec.args = vec!["-c".into(),
            "sleep 30 & echo $! > cancel.pid; (while :; do echo alive >> heartbeat; sleep .05; done) & echo ready > ready; wait".into()];
        let mut proxy = manager.spawn(spec).unwrap();
        for _ in 0..150 {
            if root.join("ready").exists() && root.join("heartbeat").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(root.join("ready").exists());
        assert!(root.join("heartbeat").exists());
        // Kill only the host proxy: closing its private connection must make
        // the broker kill the still-running command group inside the sandbox.
        proxy.kill().await.unwrap();
        let mut killed = false;
        for _ in 0..100 {
            let check = shell(&manager,
                "p=$(cat cancel.pid); test ! -r /proc/$p/stat || test \"$(awk '{print $3}' /proc/$p/stat)\" = Z").await;
            if check.status.success() {
                killed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            killed,
            "background sleep was still running after proxy disconnect"
        );
        let before = std::fs::read(root.join("heartbeat")).unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            before,
            std::fs::read(root.join("heartbeat")).unwrap(),
            "background process group still executing"
        );
    }

    pub async fn run() {
        if !std::path::Path::new("/usr/bin/bwrap").exists() {
            println!("SKIP round4 Linux sandbox cases: /usr/bin/bwrap unavailable (6 cases)");
            return;
        }
        let fixture =
            std::env::temp_dir().join(format!("ax-r4-regression-linux-{}", std::process::id()));
        let root = fixture.join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_current_dir(&root).unwrap();
        SandboxManager::configure(SandboxMode::Workspace, vec![]).unwrap();
        let mut failures = 0;
        macro_rules! check {
            ($name:literal,$future:expr) => {
                match tokio::spawn($future).await {
                    Ok(()) => println!("PASS {}", $name),
                    Err(error) => {
                        println!("FAIL {}: {error}", $name);
                        failures += 1;
                    }
                }
            };
        }
        check!(
            "sandbox_missing_read_is_not_permission_denied",
            missing_path_is_an_ordinary_error(
                root.clone(),
                "filesystem",
                json!({"operation":"read","path":"missing-read"})
            )
        );
        check!(
            "sandbox_missing_search_is_not_permission_denied",
            missing_path_is_an_ordinary_error(
                root.clone(),
                "search",
                json!({"query":"needle","path":"missing-search"})
            )
        );
        check!(
            "sandbox_missing_parent_write_is_not_permission_denied",
            missing_path_is_an_ordinary_error(
                root.clone(),
                "filesystem",
                json!({"operation":"write","path":"missing-parent/child","content":"x"})
            )
        );
        check!(
            "outside_symlink_is_a_sandbox_violation",
            outside_symlink_is_a_sandbox_violation(root.clone())
        );
        check!(
            "successful_command_keeps_background_child",
            successful_command_keeps_background_child(root.clone())
        );
        check!(
            "proxy_disconnect_kills_background_child",
            proxy_disconnect_kills_background_child(root)
        );
        std::env::set_current_dir(std::env::temp_dir()).unwrap();
        std::fs::remove_dir_all(fixture).unwrap();
        assert_eq!(failures, 0, "round4 Linux regressions failed");
    }
}

#[cfg(target_os = "linux")]
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--ax-sandbox-broker") => sandbox::run_broker(args.next().unwrap().into())
            .await
            .unwrap(),
        Some("--ax-sandbox-proxy") => {
            let socket = args.next().unwrap();
            let command = args.next().unwrap();
            std::process::exit(sandbox::run_proxy(socket.into(), &command).await.unwrap());
        }
        Some("--ax-sandbox-worker") => tool::sandbox_worker(&args.next().unwrap()).await.unwrap(),
        _ => suite::run().await,
    }
}
