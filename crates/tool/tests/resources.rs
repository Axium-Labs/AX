//! Resource declarations of the built-in tools.
//!
//! The unknown tool still gets `Resource::All` and serializes with everything;
//! every built-in tool must declare something precise instead. These tests pin
//! that contract: independent reads are never serialized by an opaque global
//! write, and a read/write overlap on the same path still conflicts.

use serde_json::json;
use tool::{
    FilesystemTool, FindFilesTool, PatchTool, Resource, ResourceAccess, ResultReader, SafetyLevel,
    SandboxMode, SearchTool, ShellTool, Tool, ToolRegistry, ViewImageTool, WebTool,
};

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::with_mode(SandboxMode::Off);
    for local in [
        std::sync::Arc::new(ShellTool) as std::sync::Arc<dyn Tool>,
        std::sync::Arc::new(FilesystemTool),
        std::sync::Arc::new(PatchTool),
        std::sync::Arc::new(FindFilesTool::new(std::path::PathBuf::from("."))),
        std::sync::Arc::new(FindFilesTool::glob(std::path::PathBuf::from("."))),
        std::sync::Arc::new(SearchTool::new(std::path::PathBuf::from("."))),
        std::sync::Arc::new(ViewImageTool::new(std::path::PathBuf::from("."))),
        std::sync::Arc::new(WebTool::new()),
    ] {
        registry.register_arc(local);
    }
    registry.register(ResultReader::default());
    registry
}

fn resources_of(
    registry: &ToolRegistry,
    name: &str,
    input: serde_json::Value,
) -> Vec<ResourceAccess> {
    registry.get(name).expect(name).resources(&input).clone()
}

fn contains_all(resources: &[ResourceAccess]) -> bool {
    resources
        .iter()
        .any(|access| access.resource == Resource::All)
}

fn conflicts(left: &[ResourceAccess], right: &[ResourceAccess]) -> bool {
    left.iter().any(|l| right.iter().any(|r| l.conflicts(r)))
}

#[test]
fn three_independent_reads_and_searches_are_never_serialized() {
    let registry = registry();
    let read_a = resources_of(
        &registry,
        "filesystem",
        json!({"operation":"read","path":"src/a.rs"}),
    );
    let search_b = resources_of(&registry, "search", json!({"query":"needle","path":"src"}));
    let find_c = resources_of(
        &registry,
        "find_files",
        json!({"pattern":"**/*.rs","path":"src"}),
    );
    for (name, resources) in [("read", &read_a), ("search", &search_b), ("find", &find_c)] {
        assert!(
            !contains_all(resources),
            "{name} must not degrade to Resource::All"
        );
        assert!(
            resources.iter().all(|access| !access.write),
            "{name} must be a read"
        );
    }
    assert!(!conflicts(&read_a, &search_b));
    assert!(!conflicts(&read_a, &find_c));
    assert!(!conflicts(&search_b, &find_c));

    // The runtime-owned reader touches no shared resource at all.
    let tool_output = resources_of(
        &registry,
        "tool_output",
        json!({"call_id":"x","start_line":1,"end_line":2}),
    );
    assert!(tool_output.is_empty());
    assert!(!conflicts(&tool_output, &read_a));
}

#[test]
fn a_read_and_a_write_on_the_same_path_conflict() {
    let registry = registry();
    let write = resources_of(
        &registry,
        "filesystem",
        json!({"operation":"write","path":"src/a.rs"}),
    );
    let read = resources_of(
        &registry,
        "filesystem",
        json!({"operation":"read","path":"src/a.rs"}),
    );
    let patch = resources_of(&registry, "patch", json!({"path":"src/a.rs"}));
    assert!(conflicts(&write, &read));
    assert!(conflicts(&patch, &read));
    assert!(conflicts(&write, &patch));
    // A write elsewhere does not conflict with the same directory's read.
    let other = resources_of(
        &registry,
        "filesystem",
        json!({"operation":"write","path":"src/b.rs"}),
    );
    assert!(!conflicts(&read, &other));
}

#[test]
fn read_only_shell_calls_do_not_serialize_each_other() {
    let registry = registry();
    let status_a = resources_of(
        &registry,
        "shell",
        json!({"command":"git status --porcelain"}),
    );
    let status_b = resources_of(&registry, "shell", json!({"command":"git status"}));
    let location = resources_of(&registry, "shell", json!({"command":"Get-Location"}));
    let version = resources_of(&registry, "shell", json!({"command":"node --version"}));
    for (name, resources) in [
        ("git status a", &status_a),
        ("git status b", &status_b),
        ("Get-Location", &location),
        ("node --version", &version),
    ] {
        assert!(
            !contains_all(resources),
            "{name} is a known read and must not be exclusive"
        );
        assert!(
            resources.iter().all(|access| !access.write),
            "{name} must be a read"
        );
    }
    assert!(!conflicts(&status_a, &status_b));
    assert!(!conflicts(&status_a, &location));
    assert!(!conflicts(&location, &version));

    // A read of a concrete path conflicts with a write of that path.
    let cat = resources_of(
        &registry,
        "shell",
        json!({"command":"Get-Content src/a.rs"}),
    );
    let write = resources_of(
        &registry,
        "filesystem",
        json!({"operation":"write","path":"src/a.rs"}),
    );
    assert!(conflicts(&cat, &write));
}

#[test]
fn unclassifiable_commands_stay_exclusive() {
    let registry = registry();
    for command in [
        "cargo test -p runtime-core",
        "git commit -m done",
        "git checkout main",
        "Get-Content a.rs > b.rs",
        "Get-Content a.rs | Select-String needle",
        "Remove-Item old.rs",
        "python -c \"open('x','w')\"",
        "Set-Content a.rs x",
        "unknown-binary --flag",
    ] {
        let resources = resources_of(&registry, "shell", json!({"command": command}));
        assert!(
            contains_all(&resources),
            "`{command}` must stay exclusive: {resources:?}"
        );
        // Exclusive effects serialize with every read too.
        let read = resources_of(
            &registry,
            "filesystem",
            json!({"operation":"read","path":"src/a.rs"}),
        );
        assert!(conflicts(&resources, &read), "`{command}` must serialize");
    }
}

#[test]
fn declared_writes_and_reads_serialize_only_where_they_overlap() {
    let registry = registry();
    let web_a = resources_of(
        &registry,
        "web",
        json!({"operation":"search","queries":["a"]}),
    );
    let web_b = resources_of(
        &registry,
        "web",
        json!({"operation":"fetch","urls":["https://x"]}),
    );
    // Web is a shared read on a named resource: it must not serialize.
    assert!(!conflicts(&web_a, &web_b));
    assert!(!contains_all(&web_a));
}

#[test]
fn every_builtin_read_tool_is_a_read_not_an_exclusive() {
    let registry = registry();
    let cases = [
        ("filesystem", json!({"operation":"read","path":"src"})),
        ("filesystem", json!({"operation":"list","path":"src"})),
        ("search", json!({"query":"needle"})),
        ("find_files", json!({"pattern":"*.rs"})),
        ("glob", json!({"pattern":"**/*.rs"})),
        ("view_image", json!({"path":"out.png"})),
        ("web", json!({"operation":"fetch","urls":["https://x"]})),
        (
            "tool_output",
            json!({"call_id":"x","start_line":1,"end_line":2}),
        ),
    ];
    for (name, input) in cases {
        let tool = registry.get(name).expect(name);
        let safety = tool.safety(&input);
        assert_eq!(safety, SafetyLevel::Safe, "{name} must be a safe read");
        assert!(
            !contains_all(&tool.resources(&input)),
            "{name} must not be exclusive"
        );
    }
}
