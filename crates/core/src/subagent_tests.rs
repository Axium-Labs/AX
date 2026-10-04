use super::*;
#[path = "../../../test/harness/subagent_continuation.rs"]
mod continuation;
use crate::{AllowAll, ApprovalPolicy, ChildCheckpoint, ChildRun, PreparedChild};
use model::{
    FunctionCall, Message, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall,
};
use std::sync::atomic::AtomicUsize;
use tool::{PermissionDecision, PermissionStore, ToolPermission, ToolRegistry};

#[derive(Default)]
struct Provider {
    requests: Mutex<Vec<ModelRequest>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}
fn answer(content: &str) -> ModelResponse {
    ModelResponse {
        content: content.into(),
        tool_calls: vec![],
        usage: None,
        finish_reason: None,
    }
}
fn call(id: &str, name: &str, input: &Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall {
            name: name.into(),
            arguments: input.to_string(),
        },
    }
}
#[async_trait]
impl ModelProvider for Provider {
    fn name(&self) -> &'static str {
        "subagent-test"
    }
    fn model_id(&self) -> &'static str {
        "inherited-model"
    }
    fn context_window(&self) -> usize {
        128_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let input = request
            .messages
            .iter()
            .find(|m| m.role == model::Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let has_results = request.messages.iter().any(|m| m.role == model::Role::Tool);
        self.requests.lock().unwrap().push(request);
        if input == "parent" {
            if has_results {
                return Ok(answer("parent final"));
            }
            return Ok(ModelResponse {
                tool_calls: (0..5)
                    .map(|i| {
                        call(
                            &format!("call-{i}"),
                            "subagent",
                            &json!({"task":format!("child-{i}"),"context":"explicit context"}),
                        )
                    })
                    .collect(),
                ..answer("")
            });
        }
        if input.contains("provider-failure") {
            return Err(ModelError::InvalidResponse("child provider failed".into()));
        }
        assert!(!input.contains("panic-worker"), "worker failure");
        if input.contains("hang") {
            std::future::pending::<()>().await;
        }
        if input.contains("attempt-denied") && !has_results {
            return Ok(ModelResponse {
                tool_calls: vec![call("denied", "write", &json!({}))],
                ..answer("")
            });
        }
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(answer("child final"))
    }
}
struct BoundTool(&'static str, Arc<AtomicUsize>);
#[async_trait]
impl Tool for BoundTool {
    fn fork_for_run(&self, _: &tool::RunContext) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Self(self.0, Arc::clone(&self.1))))
    }
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &'static str {
        "test"
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object"})
    }
    fn safety(&self, _: &Value) -> SafetyLevel {
        SafetyLevel::RequiresApproval
    }
    fn capability(&self, _: &Value) -> Capability {
        Capability::FilesystemWrite
    }
    fn resources(&self, _: &Value) -> Vec<ResourceAccess> {
        vec![]
    }
    async fn execute(&self, _: Value) -> Result<String, ToolError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Ok("wrote".into())
    }
}
#[derive(Default)]
struct Host {
    prepared: AtomicUsize,
    receipts: Arc<Mutex<Vec<crate::ChildResult>>>,
}
struct Checkpoint(Arc<Mutex<Vec<crate::ChildResult>>>);
impl ChildCheckpoint for Checkpoint {
    fn save(&mut self, _: &[Message]) -> Result<(), AgentError> {
        Ok(())
    }
    fn finish(&mut self, outcome: &mut crate::ChildResult) -> Result<(), AgentError> {
        self.0.lock().unwrap().push(outcome.clone());
        Ok(())
    }
}
#[async_trait]
impl ChildHost for Host {
    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        _: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError> {
        let n = self.prepared.fetch_add(1, Ordering::SeqCst);
        if input == "prepare-failure" {
            return Err(AgentError::Persistence("provision failed".into()));
        }
        let run = ChildRun {
            workspace_root: None,
            goal_id: format!("goal-{n}"),
            session_id: format!("session-{n}"),
            cwd: std::env::temp_dir(),
            memory_scope: format!("child-{n}"),
            state_dir: None,
            execution_budget: None,
        };
        Ok(PreparedChild {
            kernel: controller.fork_child(run.clone(), input, vec![]),
            run,
            checkpoint: Box::new(Checkpoint(Arc::clone(&self.receipts))),
            terminal: None,
        })
    }
}
fn fixture(
    enabled: bool,
    max_concurrent: usize,
) -> (AgentKernel, Arc<Provider>, Arc<Host>, Arc<AtomicUsize>) {
    let provider = Arc::new(Provider::default());
    let host = Arc::new(Host::default());
    let writes = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(BoundTool("write", Arc::clone(&writes)));
    tools.register(BoundTool("other", Arc::clone(&writes)));
    let mut kernel =
        AgentKernel::new(provider.clone(), tools, Arc::new(AllowAll)).with_child_host(host.clone());
    kernel.push_context(Message::system("parent-secret"));
    kernel.configure_subagents(SubagentConfig {
        enabled,
        max_concurrent,
        max_depth: 1,
    });
    (kernel, provider, host, writes)
}

#[tokio::test]
async fn disabled_has_no_tool_manager_initialization_or_extra_model_call() {
    let (mut kernel, provider, host, _) = fixture(false, 3);
    assert!(kernel.prepare_subagents().is_none());
    assert!(kernel.subagent_manager().is_none());
    assert!(!kernel.has_tool("subagent"));
    assert!(!kernel.has_tool("spawn_agent"));
    kernel.run_turn("plain", |_| {}).await.unwrap();
    assert_eq!(host.prepared.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(
        provider.requests.lock().unwrap()[0]
            .tools
            .iter()
            .all(|tool| !["subagent", "spawn_agent"].contains(&tool.function.name.as_str()))
    );
    assert!(kernel.subagent_manager().is_none());
}

#[tokio::test]
async fn enabled_tool_reuses_loop_with_isolated_context_session_and_final_results() {
    let (mut kernel, provider, host, _) = fixture(true, 3);
    let mut events = vec![];
    assert_eq!(
        kernel.run_turn("parent", |e| events.push(e)).await.unwrap(),
        "parent final"
    );
    assert_eq!(host.prepared.load(Ordering::SeqCst), 5);
    assert_eq!(host.receipts.lock().unwrap().len(), 5);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        7,
        "two parent requests and five child requests; no planner"
    );
    let children = requests
        .iter()
        .filter(|r| r.messages.iter().any(|m| m.content.starts_with("child-")))
        .collect::<Vec<_>>();
    assert_eq!(children.len(), 5);
    for request in children {
        assert!(
            !request
                .messages
                .iter()
                .any(|m| m.content.contains("parent-secret"))
        );
        assert!(
            request
                .messages
                .iter()
                .any(|m| m.content.contains("explicit context"))
        );
        assert!(!request.tools.iter().any(|t| {
            ["subagent", "spawn_agent", "task_queue"].contains(&t.function.name.as_str())
        }));
    }
    for message in kernel
        .take_turn_messages()
        .iter()
        .filter(|m| m.role == model::Role::Tool)
    {
        let envelope: tool::ToolResult = serde_json::from_str(&message.content).unwrap();
        let result: SubagentResult = serde_json::from_str(&envelope.raw_output).unwrap();
        assert_eq!(result.status, "completed");
        assert_eq!(result.summary, "child final");
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SubagentStarted { .. }))
            .count(),
        5
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SubagentCompleted { .. }))
            .count(),
        5
    );
    assert!(
        !events.iter().any(
            |e| matches!(e, AgentEvent::ContentDelta { delta } if delta.contains("child final"))
        )
    );
    assert!(provider.peak.load(Ordering::SeqCst) <= 3);
    assert!(provider.peak.load(Ordering::SeqCst) > 1);
}

#[tokio::test]
async fn enabled_delegation_does_not_force_a_child_or_planner_request() {
    let (mut kernel, provider, host, _) = fixture(true, 3);
    kernel.run_turn("plain", |_| {}).await.unwrap();
    assert_eq!(host.prepared.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(kernel.has_tool("subagent"));
}

#[tokio::test]
async fn named_agents_read_instructions_only_on_invocation_and_narrow_tools() {
    let (mut kernel, provider, host, _) = fixture(true, 3);
    let path = std::env::temp_dir().join(format!(
        "ax-agent-{}.md",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    kernel.configure_agent_templates(vec![AgentTemplate {
        name: "reviewer".into(),
        description: "Review changes".into(),
        instructions: path.clone(),
        tools: Some(vec!["other".into()]),
    }]);
    let _events = kernel.prepare_subagents().unwrap();
    let tool = kernel.tools.get("subagent").unwrap();
    assert_eq!(
        tool.input_schema()["properties"]["agent"]["enum"],
        json!(["reviewer"])
    );
    assert_eq!(host.prepared.load(Ordering::SeqCst), 0);
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(
        tool.execute(json!({"agent":"disabled", "task":"review"}))
            .await
            .is_err()
    );
    assert!(
        tool.execute(json!({"agent":"reviewer", "task":"review"}))
            .await
            .is_err()
    );
    assert_eq!(host.prepared.load(Ordering::SeqCst), 0);
    std::fs::write(&path, "template-instructions-loaded-on-demand").unwrap();
    assert!(
        tool.execute(json!({"agent":"reviewer", "task":"review", "tools":["write"]}))
            .await
            .is_err()
    );
    let result = tool
        .execute(json!({"agent":"reviewer", "task":"review", "context":"necessary-context"}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result).unwrap()["status"],
        "completed"
    );
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].messages.iter().any(|message| {
        message
            .content
            .contains("template-instructions-loaded-on-demand")
            && message.content.contains("necessary-context")
    }));
    assert_eq!(requests[0].tools.len(), 1);
    assert_eq!(requests[0].tools[0].function.name, "other");
    drop(requests);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn unnamed_delegation_has_no_empty_agent_enum() {
    let (mut kernel, _, _, _) = fixture(true, 3);
    let _events = kernel.prepare_subagents().unwrap();
    assert!(
        kernel.tools.get("subagent").unwrap().input_schema()["properties"]
            .get("agent")
            .is_none()
    );
}

#[tokio::test]
async fn tools_only_narrow_and_children_cannot_spawn() {
    let (mut kernel, provider, _, _) = fixture(true, 3);
    let _events = kernel.prepare_subagents().unwrap();
    let manager = kernel.subagent_manager().unwrap();
    for name in ["unknown", "subagent", "spawn_agent"] {
        assert!(
            manager
                .spawn_agent(
                    "child",
                    SpawnOptions {
                        tools: Some(vec![name.into()]),
                        ..SpawnOptions::default()
                    }
                )
                .is_err()
        );
    }
    let id = manager
        .spawn_agent(
            "child",
            SpawnOptions {
                tools: Some(vec!["write".into()]),
                ..SpawnOptions::default()
            },
        )
        .unwrap();
    assert_eq!(manager.wait_agent(&id).await.status, "completed");
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests[0]
            .tools
            .iter()
            .map(|t| t.function.name.as_str())
            .collect::<Vec<_>>(),
        vec!["write"]
    );
    assert!(
        kernel
            .fork_with_messages(vec![])
            .subagent_manager()
            .is_none()
    );
    assert!(!kernel.fork_with_messages(vec![]).has_tool("subagent"));
}

struct StoreApproval(PermissionStore);
#[async_trait]
impl ApprovalPolicy for StoreApproval {
    fn capability_decision(&self, capability: Capability) -> Option<PermissionDecision> {
        Some(self.0.decision(capability))
    }
    async fn approve(&self, _: &str, _: &Value, permission: ToolPermission) -> bool {
        self.0.decision(permission.capability) == PermissionDecision::Allow
    }
}
#[tokio::test]
async fn child_inherits_parent_permission_ceiling() {
    let (mut kernel, _, _, writes) = fixture(true, 3);
    let store = PermissionStore::default();
    store.set_capability(Capability::FilesystemWrite, PermissionDecision::Deny);
    kernel.approval = Arc::new(StoreApproval(store.clone()));
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let id = manager
        .spawn_agent("attempt-denied", SpawnOptions::default())
        .unwrap();
    let result = manager.wait_agent(&id).await;
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(result.status, "failed");
    assert!(result.error.unwrap().contains("permission denied"));
}

#[tokio::test]
async fn primitive_concurrency_is_bounded_and_wait_is_repeatable() {
    let (mut kernel, provider, _, _) = fixture(true, 2);
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let ids = (0..8)
        .map(|_| {
            manager
                .spawn_agent("child", SpawnOptions::default())
                .unwrap()
        })
        .collect::<Vec<_>>();
    for id in ids {
        assert_eq!(manager.wait_agent(&id).await.status, "completed");
        assert_eq!(manager.wait_agent(&id).await.status, "completed");
    }
    assert_eq!(provider.peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancel_timeout_and_failures_propagate_and_close_receipts() {
    let (mut kernel, provider, host, _) = fixture(true, 1);
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let id = manager
        .spawn_agent("hang", SpawnOptions::default())
        .unwrap();
    while provider.requests.lock().unwrap().is_empty() {
        tokio::task::yield_now().await;
    }
    assert!(manager.cancel_agent(&id));
    assert_eq!(manager.wait_agent(&id).await.status, "cancelled");
    assert_eq!(host.receipts.lock().unwrap().len(), 1);
    let id = manager
        .spawn_agent(
            "hang",
            SpawnOptions {
                timeout_secs: 1,
                policy: crate::ChildPolicy::default(),
                ..SpawnOptions::default()
            },
        )
        .unwrap();
    assert!(
        manager
            .wait_agent(&id)
            .await
            .error
            .unwrap()
            .contains("timed out")
    );
    for task in ["provider-failure", "prepare-failure", "panic-worker"] {
        let id = manager.spawn_agent(task, SpawnOptions::default()).unwrap();
        assert_eq!(manager.wait_agent(&id).await.status, "failed");
    }
    assert_eq!(manager.wait_agent("missing").await.status, "failed");
    assert!(!manager.cancel_agent("missing"));
}

#[tokio::test]
async fn toggle_off_removes_tool_and_manager_on_next_turn() {
    let (mut kernel, _, host, _) = fixture(true, 3);
    let _events = kernel.prepare_subagents();
    assert!(kernel.has_tool("subagent"));
    kernel.configure_subagents(SubagentConfig::default());
    kernel.run_turn("plain", |_| {}).await.unwrap();
    assert!(!kernel.has_tool("subagent"));
    assert!(kernel.subagent_manager().is_none());
    assert_eq!(host.prepared.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn zero_depth_disables_delegation_without_initialization() {
    let (mut kernel, _, host, _) = fixture(true, 3);
    kernel.configure_subagents(SubagentConfig {
        enabled: true,
        max_depth: 0,
        ..SubagentConfig::default()
    });
    assert!(kernel.prepare_subagents().is_none());
    assert!(kernel.subagent_manager().is_none());
    assert_eq!(host.prepared.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn model_tool_failure_preserves_structured_result_and_failure_status() {
    let (mut kernel, _, _, _) = fixture(true, 3);
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let tool = SubagentTool(manager);
    let Err(ToolError::Execution(raw)) = tool.execute(json!({"task":"provider-failure"})).await
    else {
        panic!("child failure must fail the tool");
    };
    let result: SubagentResult = serde_json::from_str(&raw).unwrap();
    assert_eq!(result.status, "failed");
    assert!(result.error.unwrap().contains("child provider failed"));
    let Err(ToolError::Execution(raw)) = tool
        .execute(json!({"task":"child","tools":["subagent"]}))
        .await
    else {
        panic!("recursive spawn must fail");
    };
    assert_eq!(
        serde_json::from_str::<SubagentResult>(&raw).unwrap().status,
        "failed"
    );
}

#[tokio::test]
async fn dropping_parent_tool_cancels_child_and_closes_receipt() {
    let (mut kernel, _, host, _) = fixture(true, 1);
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let tool = SubagentTool(manager.clone());
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            tool.execute(json!({"task":"hang"}))
        )
        .await
        .is_err()
    );
    assert_eq!(manager.wait_agent("subagent-1").await.status, "cancelled");
    assert_eq!(host.receipts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn admission_is_finite_and_disabling_revokes_retained_handles() {
    let (mut kernel, _, host, _) = fixture(true, 1);
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let ids = (0..64)
        .map(|_| {
            manager
                .spawn_agent("hang", SpawnOptions::default())
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(
        manager
            .spawn_agent("overflow", SpawnOptions::default())
            .is_err()
    );
    kernel.configure_subagents(SubagentConfig::default());
    assert!(
        manager
            .spawn_agent("disabled", SpawnOptions::default())
            .is_err()
    );
    for id in ids {
        assert_eq!(manager.wait_agent(&id).await.status, "cancelled");
    }
    assert_eq!(host.prepared.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn explicit_child_inheritance_and_custom_allow_cannot_elevate() {
    use crate::child_policy::*;
    let (mut kernel, provider, _, writes) = fixture(true, 1);
    kernel.push_context(Message::user("parent context marker"));
    let store = PermissionStore::default();
    store.set_capability(Capability::FilesystemWrite, PermissionDecision::Deny);
    kernel.approval = Arc::new(StoreApproval(store));
    kernel.constrain_permissions(tool::PermissionProfile::default());
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let policy = ChildPolicy {
        context: ContextInheritance::Full,
        tools: Selection::None,
        ..Default::default()
    };
    let id = manager
        .spawn_agent(
            "child",
            SpawnOptions {
                policy,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(manager.wait_agent(&id).await.status, "completed");
    {
        let requests = provider.requests.lock().unwrap();
        assert!(requests[0].tools.is_empty());
        assert!(
            requests[0]
                .messages
                .iter()
                .any(|m| m.content.contains("parent context marker"))
        );
    }
    let policy = ChildPolicy {
        permissions: PermissionInheritance::Custom,
        custom_permissions: tool::PermissionProfile {
            rules: vec![tool::PermissionRule {
                decision: PermissionDecision::Allow,
                matcher: tool::RuleMatcher::ToolParameter {
                    tool: "write".into(),
                    pointer: String::new(),
                    pattern: "*".into(),
                },
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let id = manager
        .spawn_agent(
            "attempt-denied",
            SpawnOptions {
                policy,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(manager.wait_agent(&id).await.status, "failed");
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn child_model_override_requires_parent_registration() {
    use crate::child_policy::*;
    let (mut kernel, _, _, _) = fixture(true, 1);
    let alternate = Arc::new(Provider::default());
    kernel.register_child_model("approved".into(), alternate.clone());
    let _events = kernel.prepare_subagents();
    let manager = kernel.subagent_manager().unwrap();
    let mut policy = ChildPolicy {
        model: ModelInheritance::Override,
        model_override: Some("unknown".into()),
        ..Default::default()
    };
    assert!(
        manager
            .spawn_agent(
                "child",
                SpawnOptions {
                    policy: policy.clone(),
                    ..Default::default()
                }
            )
            .is_err()
    );
    policy.model_override = Some("approved".into());
    let id = manager
        .spawn_agent(
            "child",
            SpawnOptions {
                policy,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(manager.wait_agent(&id).await.status, "completed");
    assert_eq!(alternate.requests.lock().unwrap().len(), 1);
}
