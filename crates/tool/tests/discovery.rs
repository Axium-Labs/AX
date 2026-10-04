//! Discovery tools must be one shared implementation: the main agent and a
//! child agent get the same names, schema and behavior, while a child stays
//! inside its own workspace.
//!
//! Isolation here comes from the binding (cwd) applied by
//! `ToolRegistry::fork_for_run`, not from a second search implementation.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tool::{FilesystemTool, FindFilesTool, RunContext, SandboxMode, SearchTool, ToolRegistry};

fn temp(label: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "ax-discovery-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn context(cwd: &Path) -> RunContext {
    RunContext {
        workspace_root: cwd.to_path_buf(),
        cwd: cwd.to_path_buf(),
        state_dir: cwd.join(".ax"),
        session_id: "child-session".into(),
        memory_scope: "session".into(),
        input: "locate the event log".into(),
    }
}

/// The one registry both agents use. `SandboxMode::Off` keeps the test
/// in-process; the binding under test is the same one used with confinement.
fn registry(workspace: &Path) -> ToolRegistry {
    let mut registry = ToolRegistry::with_mode(SandboxMode::Off);
    registry.register(FindFilesTool::new(workspace.to_path_buf()));
    registry.register(FindFilesTool::glob(workspace.to_path_buf()));
    registry.register(SearchTool::new(workspace.to_path_buf()));
    registry.register(FilesystemTool);
    registry
}

async fn json_call(registry: &ToolRegistry, name: &str, input: Value) -> Value {
    let output = text_call(registry, name, input).await;
    serde_json::from_str(&output).expect("discovery tools return JSON")
}

async fn text_call(registry: &ToolRegistry, name: &str, input: Value) -> String {
    registry
        .get(name)
        .unwrap_or_else(|| panic!("{name} is registered"))
        .execute(input)
        .await
        .unwrap_or_else(|error| panic!("{name} failed: {error}"))
}

#[tokio::test]
async fn main_and_child_agents_share_identical_discovery_tools() {
    let root = temp("parity");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/lib.rs"), "fn needle() {}\n").unwrap();
    std::fs::write(workspace.join("src/events.jsonl"), "{\"event\":1}\n").unwrap();

    let shared = registry(&workspace);
    let child = shared.fork_for_run(&context(&workspace));

    assert_eq!(shared.names(), child.names());
    for name in shared.names() {
        let (main, forked) = (shared.get(name).unwrap(), child.get(name).unwrap());
        assert_eq!(main.input_schema(), forked.input_schema(), "{name} schema");
        assert_eq!(
            main.description(),
            forked.description(),
            "{name} description"
        );
        assert_eq!(
            main.capability(&json!({})),
            forked.capability(&json!({})),
            "{name}"
        );
    }

    let main_find = json_call(&shared, "find_files", json!({"pattern":"**/*.jsonl"})).await;
    let child_find = json_call(&child, "glob", json!({"pattern":"**/*.jsonl"})).await;
    assert_eq!(main_find["paths"], json!(["src/events.jsonl"]));
    assert_eq!(child_find["paths"], main_find["paths"]);

    let main_search = json_call(&shared, "search", json!({"query":"needle"})).await;
    let child_search = json_call(&child, "search", json!({"query":"needle"})).await;
    assert_eq!(main_search["matches"], child_search["matches"]);
    assert_eq!(child_search["matches"][0]["path"], "src/lib.rs");

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_child_only_reaches_its_own_workspace() {
    let root = temp("isolation");
    let parent = root.join("parent");
    let child_root = root.join("children/run/workspace");
    std::fs::create_dir_all(&parent).unwrap();
    std::fs::create_dir_all(&child_root).unwrap();
    std::fs::write(parent.join("parent-only.txt"), "needle-parent\n").unwrap();
    std::fs::write(child_root.join("child-only.txt"), "needle-child\n").unwrap();

    let shared = registry(&parent);
    let child = shared.fork_for_run(&context(&child_root));

    // No `path` at all: the child's default scope is its own workspace.
    let search = json_call(&child, "search", json!({"query":"needle"})).await;
    let paths: Vec<&str> = search["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["child-only.txt"]);

    let found = json_call(&child, "find_files", json!({"pattern":"**/*.txt"})).await;
    assert_eq!(found["paths"], json!(["child-only.txt"]));

    // The same query over the parent workspace does see the parent file, so the
    // child's result is isolation rather than an empty tree.
    let parent_search = json_call(&shared, "search", json!({"query":"needle"})).await;
    assert_eq!(parent_search["count"], 1);
    assert_eq!(parent_search["matches"][0]["path"], "parent-only.txt");

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn child_reads_and_lists_are_bound_to_the_child_workspace() {
    let root = temp("read-binding");
    let child_root = root.join("children/run/workspace");
    std::fs::create_dir_all(child_root.join("src")).unwrap();
    std::fs::write(child_root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let child = registry(&child_root).fork_for_run(&context(&child_root));

    // Relative paths resolve against the child workspace, not the process cwd.
    assert_eq!(
        text_call(
            &child,
            "filesystem",
            json!({"operation":"read","path":"src/main.rs"})
        )
        .await,
        "fn main() {}\n"
    );
    assert_eq!(
        text_call(&child, "filesystem", json!({"operation":"list","path":"."})).await,
        "src"
    );
    assert_eq!(
        text_call(
            &child,
            "filesystem",
            json!({"operation":"list","path":"src"})
        )
        .await,
        "main.rs"
    );

    let missing = child
        .get("filesystem")
        .unwrap()
        .execute(json!({"operation":"read","path":"src/absent.rs"}))
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("absent.rs"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn a_child_locates_a_nested_jsonl_without_a_recursive_shell_scan() {
    let root = temp("jsonl");
    let child_root = root.join("children/run/workspace");
    std::fs::create_dir_all(child_root.join("out/sessions")).unwrap();
    std::fs::write(
        child_root.join("out/sessions/2026-10-02.jsonl"),
        "{\"kind\":\"turn\"}\n",
    )
    .unwrap();

    let child = registry(&child_root).fork_for_run(&context(&child_root));
    let found = json_call(
        &child,
        "find_files",
        json!({"path":".","pattern":"**/*.jsonl"}),
    )
    .await;
    assert_eq!(found["paths"], json!(["out/sessions/2026-10-02.jsonl"]));
    assert_eq!(found["root"], ".");

    std::fs::remove_dir_all(root).unwrap();
}
