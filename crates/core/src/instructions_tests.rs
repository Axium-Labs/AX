//! Resolution-order and provenance coverage for project instructions.

use super::{
    AGENTS_FILE, CONTEXT_PREFIX, InstructionResolver, InstructionScope, OVERRIDE_FILE, RULES_DIR,
    glob_match, summarize,
};
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ax-instructions-{}", uuid_like()));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn resolver(&self) -> InstructionResolver {
        InstructionResolver::new(self.root.clone(), None)
    }

    fn cwd(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

#[test]
fn repository_root_instructions_apply_from_any_directory() {
    let fixture = Fixture::new();
    let root_file = fixture.write(AGENTS_FILE, "root rules");
    fixture.write("src/nested/file.rs", "content");
    let resolution = fixture.resolver().resolve_cwd(&fixture.cwd("src/nested"));
    assert_eq!(resolution.segments.len(), 1);
    let segment = &resolution.segments[0];
    assert_eq!(segment.scope, InstructionScope::Repository);
    assert_eq!(segment.source_path, root_file);
    assert_eq!(segment.provenance, "repository root");
    assert_eq!(segment.content.trim(), "root rules");
}

#[test]
fn nested_instructions_load_after_the_root_and_win_on_priority() {
    let fixture = Fixture::new();
    fixture.write(AGENTS_FILE, "root rules");
    fixture.write("src/AGENTS.md", "src rules");
    fixture.write("src/api/AGENTS.md", "api rules");
    let cwd = fixture.cwd("src/api");
    let resolution = fixture.resolver().resolve_cwd(&cwd);
    let contents: Vec<_> = resolution
        .segments
        .iter()
        .map(|segment| segment.content.trim().to_owned())
        .collect();
    assert_eq!(contents, ["root rules", "src rules", "api rules"]);
    let priorities: Vec<_> = resolution.segments.iter().map(|s| s.priority).collect();
    assert!(priorities.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        resolution.segments[2].scope,
        InstructionScope::Directory {
            path: PathBuf::from("src").join("api")
        }
    );
    assert!(resolution.segments[2].provenance.contains("level 2"));
    // A sibling directory is not on the chain.
    resolution
        .segments
        .iter()
        .for_each(|segment| assert!(!segment.content.contains("web rules")));
}

#[test]
fn override_replaces_the_same_level_file() {
    let fixture = Fixture::new();
    fixture.write(AGENTS_FILE, "root rules");
    fixture.write("src/AGENTS.md", "src rules");
    fixture.write("src/AGENTS.override.md", "src override");
    let resolution = fixture.resolver().resolve_cwd(&fixture.cwd("src"));
    let contents: Vec<_> = resolution
        .segments
        .iter()
        .map(|segment| segment.content.trim().to_owned())
        .collect();
    assert_eq!(contents, ["root rules", "src override"]);
    assert!(
        resolution.segments[1]
            .source_path
            .ends_with(Path::new(OVERRIDE_FILE))
    );
}

#[test]
fn cwd_outside_the_root_only_yields_the_repository_layer() {
    let fixture = Fixture::new();
    fixture.write(AGENTS_FILE, "root rules");
    let outside = fixture
        .root
        .join("..")
        .join(format!("ax-outside-{}", uuid_like()));
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join(AGENTS_FILE), "outsider rules").unwrap();
    let resolution = fixture.resolver().resolve_cwd(&outside);
    assert_eq!(resolution.segments.len(), 1);
    assert_eq!(resolution.segments[0].content.trim(), "root rules");
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn path_scoped_rules_apply_only_to_matching_targets() {
    let fixture = Fixture::new();
    fixture.write(AGENTS_FILE, "root rules");
    fixture.write("src/api/handler.rs", "fn handler() {}");
    fixture.write("src/web/page.ts", "export const page = 1;");
    fixture.write(
        &format!("{RULES_DIR}/api.md"),
        "---\napplyTo: src/api/**/*.rs\n---\nship checked errors",
    );
    fixture.write(
        &format!("{RULES_DIR}/web.md"),
        "---\npaths:\n  - src/web/**\n---\nkeep bundles small",
    );
    let resolver = fixture.resolver();
    let api = resolver.resolve(
        &fixture.cwd("src/api"),
        &[fixture.root.join("src/api/handler.rs")],
    );
    let labels: Vec<_> = api
        .segments
        .iter()
        .map(super::InstructionSegment::scope_label)
        .collect();
    assert!(labels.iter().any(|label| label == "path src/api/**/*.rs"));
    assert!(!labels.iter().any(|label| label.contains("src/web")));
    let api_text = api.render(4_096).0;
    assert!(api_text.contains("ship checked errors"));
    assert!(!api_text.contains("keep bundles small"));

    let web = resolver.resolve(
        &fixture.cwd("src/web"),
        &[fixture.root.join("src/web/page.ts")],
    );
    let web_text = web.render(4_096).0;
    assert!(web_text.contains("keep bundles small"));
    assert!(!web_text.contains("ship checked errors"));

    // No target means no path-scoped rule is loaded at all.
    let none = resolver.resolve_cwd(&fixture.cwd("src/api"));
    assert!(
        none.segments
            .iter()
            .all(|segment| !matches!(segment.scope, InstructionScope::Path { .. }))
    );
}

#[test]
fn resolution_is_deterministic_and_independent_of_memory() {
    let fixture = Fixture::new();
    fixture.write(AGENTS_FILE, "root rules");
    fixture.write("src/AGENTS.md", "src rules");
    fixture.write(
        &format!("{RULES_DIR}/a.md"),
        "---\napplyTo: src/**/*.rs\n---\nalpha",
    );
    fixture.write(
        &format!("{RULES_DIR}/b.md"),
        "---\napplyTo: src/**/*.rs\n---\nbeta",
    );
    let resolver = fixture.resolver();
    let cwd = fixture.cwd("src");
    let target = fixture.root.join("src/lib.rs");
    let first = resolver.resolve(&cwd, std::slice::from_ref(&target));
    // Two runs, and a run with a differently ordered target list, agree.
    let second = resolver.resolve(&cwd, std::slice::from_ref(&target));
    let third = resolver.resolve(&cwd, &[fixture.root.join("src/other.rs"), target.clone()]);
    let project = |resolution: &super::InstructionResolution| {
        resolution
            .segments
            .iter()
            .map(|segment| {
                (
                    segment.source_path.clone(),
                    segment.priority,
                    segment.provenance.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(project(&first), project(&second));
    assert_eq!(project(&first), project(&third));
    // Resolution is a pure function of the filesystem: nothing is recalled.
    let groups = summarize(&first);
    assert_eq!(groups.get("repository"), Some(&1));
    assert_eq!(groups.get("directory src"), Some(&1));
}

#[test]
fn instruction_budget_keeps_the_chain_and_the_most_specific_rules() {
    let fixture = Fixture::new();
    fixture.write(AGENTS_FILE, "root rules");
    let body = "x".repeat(20_000);
    fixture.write(
        &format!("{RULES_DIR}/broad.md"),
        &format!("---\napplyTo: src/**\n---\n{body}"),
    );
    fixture.write(
        &format!("{RULES_DIR}/narrow.md"),
        "---\napplyTo: src/api/**/*.rs\n---\nnarrow rule",
    );
    let resolution = fixture.resolver().resolve(
        &fixture.cwd("src/api"),
        &[fixture.root.join("src/api/handler.rs")],
    );
    let (rendered, omitted) = resolution.render(200);
    assert!(rendered.contains("root rules"));
    assert!(rendered.contains("narrow rule"));
    assert!(rendered.contains("omitted for the instruction token budget"));
    assert_eq!(omitted, 1);
}

#[test]
fn rendered_message_carries_source_provenance_for_every_segment() {
    let fixture = Fixture::new();
    fixture.write(AGENTS_FILE, "root rules");
    fixture.write("src/AGENTS.md", "src rules");
    fixture.write(
        &format!("{RULES_DIR}/api.md"),
        "---\napplyTo: src/api/*.rs\n---\napi rule",
    );
    let resolution = fixture.resolver().resolve(
        &fixture.cwd("src/api"),
        &[fixture.root.join("src/api/handler.rs")],
    );
    let message = resolution.message(4_096).expect("instructions apply");
    assert!(message.content.starts_with(CONTEXT_PREFIX));
    assert_eq!(message.role, model::Role::System);
    assert!(message.content.contains("provenance=\"repository root\""));
    assert!(
        message
            .content
            .contains("provenance=\"directory src (level 1)\"")
    );
    assert!(
        message
            .content
            .contains("provenance=\"path-scoped rule matched by `src/api/*.rs`\"")
    );
    assert_eq!(resolution.segments.len(), 3);
}

#[test]
fn glob_matching_handles_recursive_and_single_star_segments() {
    assert!(glob_match("src/**/*.rs", "src/lib.rs"));
    assert!(glob_match("src/**/*.rs", "src/api/handler.rs"));
    assert!(!glob_match("src/**/*.rs", "src/web/page.ts"));
    assert!(glob_match("**/*.jsonl", "a/b/events.jsonl"));
    assert!(!glob_match("src/*.rs", "src/api/handler.rs"));
    assert!(glob_match("src/?.rs", "src/a.rs"));
    assert!(!glob_match("src/?.rs", "src/ab.rs"));
}

#[test]
fn absent_files_produce_no_message() {
    let fixture = Fixture::new();
    let resolution = fixture.resolver().resolve_cwd(&fixture.root);
    assert!(resolution.is_empty());
    assert!(resolution.message(4_096).is_none());
}
