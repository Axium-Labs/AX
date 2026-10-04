//! Durable-receipt recovery: a child that finished in an earlier process must
//! be recognised, never re-run.

use super::*;

#[test]
fn assistant_final_message_without_pending_calls_is_a_completed_receipt() {
    let messages = vec![
        Message::user("do the thing"),
        Message::assistant("all done", vec![]),
    ];
    let result = terminal_result(&messages).expect("terminal");
    assert_eq!(result.status, ChildStatus::Completed);
    assert_eq!(result.summary, "all done");
    assert!(result.diagnostics.is_empty());
}

#[test]
fn unresolved_tool_failure_turns_the_receipt_failed() {
    let failure = serde_json::json!({"status": "error", "raw_output": "compile error"});
    let messages = vec![
        Message::user("do the thing"),
        Message::assistant(
            String::new(),
            vec![model::ToolCall {
                id: "c1".into(),
                kind: "function".into(),
                function: model::FunctionCall {
                    name: "shell".into(),
                    arguments: "{}".into(),
                },
            }],
        ),
        Message::tool("c1", failure.to_string()),
        Message::assistant("I could not finish", vec![]),
    ];
    let result = terminal_result(&messages).expect("terminal");
    assert_eq!(result.status, ChildStatus::Failed);
    assert!(result.summary.starts_with("I could not finish"));
    assert!(
        result
            .failure_reason
            .as_deref()
            .unwrap_or_default()
            .contains("Unresolved tool failures")
    );
    assert!(result.diagnostics.contains(&"compile error".to_owned()));
}

#[test]
fn a_trailing_pending_call_is_not_terminal() {
    let messages = vec![
        Message::user("do the thing"),
        Message::assistant(
            "working",
            vec![model::ToolCall {
                id: "c1".into(),
                kind: "function".into(),
                function: model::FunctionCall {
                    name: "search".into(),
                    arguments: "{}".into(),
                },
            }],
        ),
    ];
    assert!(terminal_result(&messages).is_none());
}

#[test]
fn a_trailing_user_message_is_not_terminal() {
    let messages = vec![Message::assistant("done", vec![]), Message::user("again")];
    assert!(terminal_result(&messages).is_none());
}

#[test]
fn forked_children_share_the_controller_registry_but_not_its_scope() {
    use std::sync::Arc;
    use tool::ToolRegistry;

    struct MockProvider;
    #[async_trait::async_trait]
    impl model::ModelProvider for MockProvider {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn model_id(&self) -> &'static str {
            "mock"
        }
        fn context_window(&self) -> usize {
            10_000
        }
        async fn complete(
            &self,
            _: model::ModelRequest,
        ) -> Result<model::ModelResponse, model::ModelError> {
            Ok(model::ModelResponse::default())
        }
    }

    let mut registry = ToolRegistry::with_mode(tool::SandboxMode::Off);
    registry.register(tool::SandboxedTool::new(
        std::sync::Arc::new(tool::FilesystemTool),
        std::env::temp_dir(),
    ));
    let kernel = AgentKernel::new(Arc::new(MockProvider), registry, Arc::new(crate::AllowAll));
    let run = ChildRun {
        workspace_root: None,
        goal_id: "child-1".into(),
        session_id: "child-1".into(),
        cwd: std::env::temp_dir(),
        memory_scope: "child:1".into(),
        state_dir: None,
        execution_budget: None,
    };
    let child = kernel.fork_child(run.clone(), "task input", Vec::new());
    // Same registry, rebound to the child's scope: no second implementation of
    // search, filesystem, patch, shell or permissions.
    assert_eq!(child.tools.names(), kernel.tools.names());
    assert!(child.has_tool("filesystem"));
    assert_eq!(child.child_run.as_ref(), Some(&run));
    // A child cannot delegate further.
    assert!(child.child_host.is_none());
    // Context is the child's own, not the controller's.
    assert_eq!(child.messages().len(), 0);
}
