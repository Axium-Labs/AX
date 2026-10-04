use super::*;
use model::{FunctionCall, ModelError, ModelProvider, ModelRequest, ModelResponse, ToolCall};
use runtime_core::{AgentEvent, AllowAll, ExecutionBudget, QueueState, task_queue::TaskStatus};
use serde_json::{Value, json};
use std::sync::Arc;

fn git_test(path: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
fn call(name: &str, input: Value) -> ModelResponse {
    ModelResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: name.into(),
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
    tasks: Value,
    revisions: Vec<String>,
}
#[async_trait]
impl ModelProvider for Provider {
    fn name(&self) -> &'static str {
        "workspace-fixture"
    }
    fn model_id(&self) -> &'static str {
        "fixture"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        assert!(
            !request
                .messages
                .iter()
                .any(|m| m.content.starts_with("[ax-completion-review]")),
            "default must not invoke reviewer"
        );
        if request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-child-runtime]"))
        {
            let task = request
                .messages
                .iter()
                .find(|m| m.role == model::Role::User)
                .unwrap()
                .content
                .clone();
            let index = if task == "repo A" { 0 } else { 1 };
            let phase = request
                .messages
                .iter()
                .filter(|m| m.role == model::Role::Tool)
                .count();
            if phase == 0 {
                let prepared = request
                    .messages
                    .iter()
                    .find(|message| message.content.starts_with("[ax-task-workspace]"))
                    .unwrap();
                let prepared: Value =
                    serde_json::from_str(prepared.content.split_once('\n').unwrap().1).unwrap();
                assert_eq!(prepared["workspace"]["mode"], "git");
                assert_eq!(prepared["workspace"]["revision"], self.revisions[index]);
                assert_eq!(prepared["prepared"], true);
                let environment = request
                    .messages
                    .iter()
                    .find(|m| m.content.starts_with("[ax-environment]"))
                    .unwrap();
                let environment: Value =
                    serde_json::from_str(environment.content.split_once('\n').unwrap().1).unwrap();
                assert!(
                    Path::new(environment["cwd"].as_str().unwrap())
                        .join("code.txt")
                        .exists()
                );
                return Ok(call("shell", json!({"command":"git rev-parse HEAD"})));
            }
            if phase == 1 {
                let output = request
                    .messages
                    .iter()
                    .find(|m| m.role == model::Role::Tool)
                    .unwrap();
                assert!(
                    output.content.contains(&self.revisions[index]),
                    "{}",
                    output.content
                );
                return Ok(call(
                    "filesystem",
                    json!({"operation":"write","path":"code.txt","content":format!("fixed {task}\n")}),
                ));
            }
            if phase == 2 {
                let output = request
                    .messages
                    .iter()
                    .find(|m| m.content.starts_with("[ax-artifact-output]"))
                    .unwrap();
                assert!(output.content.contains("staging directory"));
                return Ok(call(
                    "filesystem",
                    json!({"operation":"write","path":if index==0 {"../.ax-artifacts/evaluation.json"} else {".ax-artifacts/evaluation.json"},"content":"{\"official_resolved\":null}"}),
                ));
            }
        } else if !request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-task-summary]"))
        {
            return Ok(call(
                "task_queue",
                json!({"action":"start","execution":"children","overall_goal":"independent repositories","tasks":self.tasks}),
            ));
        }
        Ok(ModelResponse {
            content: "finished".into(),
            tool_calls: vec![],
            usage: None,
            finish_reason: None,
        })
    }
}

#[tokio::test]
async fn two_repositories_exact_revisions_partial_setup_failure_and_durable_patches() {
    let root = std::env::temp_dir().join(format!("ax-workspaces-{}", uuid::Uuid::new_v4()));
    let source = root.join("controller");
    std::fs::create_dir_all(&source).unwrap();
    let mut repositories = Vec::new();
    let mut revisions = Vec::new();
    for name in ["A", "B"] {
        let repo = root.join(name);
        std::fs::create_dir_all(&repo).unwrap();
        git_test(&repo, &["init"]);
        git_test(&repo, &["config", "user.email", "test@example.invalid"]);
        git_test(&repo, &["config", "user.name", "Fixture"]);
        std::fs::write(repo.join("code.txt"), format!("before {name}\n")).unwrap();
        if name == "A" {
            std::fs::create_dir_all(repo.join("pkg")).unwrap();
            std::fs::write(repo.join("pkg/code.txt"), "before A\n").unwrap();
        }
        git_test(&repo, &["add", "."]);
        git_test(&repo, &["commit", "-m", "base"]);
        let old = git_test(&repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("code.txt"), format!("new {name}\n")).unwrap();
        git_test(&repo, &["add", "."]);
        git_test(&repo, &["commit", "-m", "next"]);
        revisions.push(if name == "A" {
            old
        } else {
            git_test(&repo, &["rev-parse", "HEAD"])
        });
        repositories.push(repo);
    }
    let output = source.join("output");
    let tasks = json!([
        {"title":"unavailable","input":"unavailable","workspace":{"mode":"git","repo_url":root.join("missing"),"revision":"HEAD"}},
        {"title":"repo A","input":"repo A","workspace":{"mode":"git","repo_url":repositories[0],"revision":revisions[0],"subdir":"pkg"},"output_dir":output.join("A")},
        {"title":"repo B","input":"repo B","workspace":{"mode":"git","repo_url":repositories[1],"revision":revisions[1]},"output_dir":"output/B"}
    ]);
    let host = Arc::new(LocalChildHost {
        sandbox: std::sync::OnceLock::new(),
        source: source.clone(),
        root: root.join("children"),
        excluded: vec![],
        policy: WorkspacePolicy::default(),
    });
    let mut tools = tool::ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(tool::ShellTool);
    tools.register(tool::FilesystemTool);
    let mut kernel = AgentKernel::new(
        Arc::new(Provider { tasks, revisions }),
        tools,
        Arc::new(AllowAll),
    )
    .with_coding_harness()
    .with_child_host(host)
    .with_execution_scope(source)
    .with_execution_budget(ExecutionBudget {
        max_steps: 20,
        ..ExecutionBudget::default()
    });
    let mut starts = 0;
    kernel
        .run_turn("Repair each repository", |e| {
            if matches!(e,AgentEvent::ToolStarted {name,..} if name=="shell") {
                starts += 1;
            }
        })
        .await
        .unwrap();
    let queue = kernel.task_queue().unwrap();
    assert_eq!(queue.state, QueueState::Completed);
    assert_eq!(queue.tasks[0].status, TaskStatus::Failed);
    assert_eq!(starts, 2, "{:#?}", queue.tasks);
    for (name, task) in ["A", "B"].into_iter().zip(&queue.tasks[1..]) {
        assert_eq!(
            task.status,
            TaskStatus::Completed,
            "{:?}",
            task.failure_reason
        );
        let run = task.child.as_ref().unwrap();
        assert!(!run.cwd.exists());
        if name == "A" {
            assert_eq!(run.cwd.file_name().unwrap(), "pkg");
            assert_eq!(run.workspace_root().file_name().unwrap(), "workspace");
        }
        let patch = std::fs::read_to_string(output.join(name).join("final.patch")).unwrap();
        assert!(patch.contains("+fixed repo"));
        assert!(!patch.contains("official_resolved"));
        let evaluation: Value = serde_json::from_slice(
            &std::fs::read(output.join(name).join("evaluation.json")).unwrap(),
        )
        .unwrap();
        assert!(evaluation["official_resolved"].is_null());
        let result: ChildResult =
            serde_json::from_slice(&std::fs::read(output.join(name).join("result.json")).unwrap())
                .unwrap();
        assert_eq!(result.changed_files.len(), 1);
        assert_eq!(result.diff_stat.files, 1);
        assert!(
            result.artifacts.iter().any(|artifact| {
                artifact.kind == "output"
                    && Path::new(&artifact.path).file_name().unwrap() == "evaluation.json"
            }),
            "receipt lists this run's exported report instead of relying on old output files"
        );
        assert!(
            result
                .artifacts
                .iter()
                .all(|artifact| Path::new(&artifact.path).exists())
        );
        assert!(run.state_dir.as_ref().unwrap().join("result.json").exists());
    }
    // Source repository modifications never leak from isolated worktrees.
    assert_eq!(
        std::fs::read_to_string(repositories[0].join("code.txt")).unwrap(),
        "new A\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}

struct NoGitProvider {
    tasks: Value,
}
#[async_trait]
impl ModelProvider for NoGitProvider {
    fn name(&self) -> &'static str {
        "nongit-fixture"
    }
    fn model_id(&self) -> &'static str {
        "fixture"
    }
    fn context_window(&self) -> usize {
        100_000
    }
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        assert!(
            !request
                .messages
                .iter()
                .any(|m| m.content.starts_with("[ax-completion-review]")),
            "default must not invoke reviewer"
        );
        if request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-child-runtime]"))
        {
            if !request.messages.iter().any(|m| m.role == model::Role::Tool) {
                return Ok(call(
                    "shell",
                    json!({"command":if cfg!(windows) {"Set-Content -LiteralPath code.txt -Value fixed -Encoding utf8"}else{"printf 'fixed\n' > code.txt"}}),
                ));
            }
        } else if !request
            .messages
            .iter()
            .any(|m| m.content.starts_with("[ax-task-summary]"))
        {
            assert!(
                request
                    .tools
                    .iter()
                    .any(|t| t.function.name == "child_result")
            );
            return Ok(call(
                "task_queue",
                json!({"action":"start","execution":"children","overall_goal":"create and edit ordinary files","tasks":self.tasks}),
            ));
        }
        Ok(ModelResponse {
            content: "done".into(),
            tool_calls: vec![],
            usage: None,
            finish_reason: None,
        })
    }
}
#[tokio::test]
async fn empty_and_non_git_inherited_shell_edits_survive_cleanup_without_fabricated_diff_counts() {
    let root = std::env::temp_dir().join(format!("ax-nongit-{}", uuid::Uuid::new_v4()));
    let source = root.join("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("code.txt"), "before\n").unwrap();
    let output = root.join("output");
    for name in ["new", "existing"] {
        std::fs::create_dir_all(output.join(name)).unwrap();
        std::fs::write(
            output.join(name).join("evaluation.json"),
            r#"{"stale":true}"#,
        )
        .unwrap();
        std::fs::write(
            output.join(name).join("result.json"),
            r#"{"stale_previous_run":true}"#,
        )
        .unwrap();
    }
    let tasks = json!([
        {"title":"new","input":"create new file","workspace":{"mode":"empty"},"output_dir":output.join("new")},
        {"title":"existing","input":"edit existing file","workspace":{"mode":"inherit"},"output_dir":output.join("existing")}
    ]);
    let host = Arc::new(LocalChildHost {
        sandbox: std::sync::OnceLock::new(),
        source: source.clone(),
        root: root.join(".ax/children"),
        excluded: vec![],
        policy: WorkspacePolicy::default(),
    });
    let mut tools = tool::ToolRegistry::with_mode(tool::SandboxMode::Off);
    tools.register(tool::ShellTool);
    let mut runtime =
        AgentKernel::new(Arc::new(NoGitProvider { tasks }), tools, Arc::new(AllowAll))
            .with_coding_harness()
            .with_child_host(host)
            .with_execution_scope(source.clone());
    runtime.run_turn("write files", |_| {}).await.unwrap();
    let queue = runtime.task_queue().unwrap();
    for (index, name) in ["new", "existing"].iter().enumerate() {
        assert_eq!(queue.tasks[index].status, TaskStatus::Completed);
        assert!(!queue.tasks[index].child.as_ref().unwrap().cwd.exists());
        let saved: ChildResult = serde_json::from_slice(
            &std::fs::read(output.join(name).join("child_result.json")).unwrap(),
        )
        .unwrap();
        let manifest: Value = serde_json::from_slice(
            &std::fs::read(output.join(name).join("artifact-manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["child_id"], saved.child_id);
        for name in [
            "result.json",
            "child_result.json",
            "trace.jsonl",
            "metrics.json",
            "final.patch",
        ] {
            assert!(
                manifest["artifacts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|a| Path::new(a["path"].as_str().unwrap()).file_name().unwrap() == name),
                "host output {name} must be current evidence"
            );
        }
        assert!(
            !manifest["artifacts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["path"].as_str().unwrap().ends_with("evaluation.json")),
            "old evaluation is not current evidence"
        );
        assert_eq!(saved.changed_files.len(), 1);
        let current: ChildResult =
            serde_json::from_slice(&std::fs::read(output.join(name).join("result.json")).unwrap())
                .unwrap();
        assert_eq!(
            current.child_id, saved.child_id,
            "previous output is not this run's result"
        );
        assert_eq!(
            saved.changed_files[0].change,
            if index == 0 { "created" } else { "modified" }
        );
        assert_eq!(saved.diff_stat.insertions, None);
        assert_eq!(saved.diff_stat.deletions, None);
        assert!(
            std::fs::read_to_string(output.join(name).join("changed_files/code.txt"))
                .unwrap()
                .contains("fixed")
        );
    }
    assert_eq!(
        std::fs::read_to_string(source.join("code.txt")).unwrap(),
        "before\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}
