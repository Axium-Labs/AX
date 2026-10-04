//! Structured receipt coverage: compact projection, durable full record, and
//! the read interface the controller uses to fetch detail on demand.

use super::*;
use crate::AgentEvent;

fn event_tool_started(id: &str, name: &str, input: Value) -> AgentEvent {
    AgentEvent::ToolStarted {
        id: id.into(),
        name: name.into(),
        detail: String::new(),
        input,
    }
}

fn event_tool_finished(id: &str, name: &str, success: bool, summary: &str) -> AgentEvent {
    AgentEvent::ToolFinished {
        id: id.into(),
        name: name.into(),
        success,
        diagnostics: Vec::new(),
        result: tool::ToolResult::new(success, summary.to_owned()),
    }
}

#[test]
fn observer_collects_files_validation_findings_and_metrics() {
    let mut observer = ChildObserver::new();
    observer.observe(&AgentEvent::ModelStarted {
        provider: "p".into(),
        model: "m".into(),
    });
    observer.observe(&event_tool_started(
        "1",
        "search",
        json!({"query": "needle"}),
    ));
    observer.observe(&event_tool_finished("1", "search", true, "3 matches"));
    observer.observe(&event_tool_started(
        "2",
        "patch",
        json!({"path": "src/api/handler.rs"}),
    ));
    observer.observe(&event_tool_finished("2", "patch", true, "applied"));
    observer.observe(&event_tool_started(
        "3",
        "shell",
        json!({"command": "cargo test -p runtime-core"}),
    ));
    observer.observe(&event_tool_finished("3", "shell", false, "2 tests failed"));
    observer.observe(&AgentEvent::ModelStarted {
        provider: "p".into(),
        model: "m".into(),
    });
    observer.observe(&AgentEvent::ContentDelta {
        delta: "done".into(),
    });
    let result = observer.into_result("child-1", "task-1", ChildStatus::Completed, None);

    assert_eq!(result.metrics.model_rounds, 2);
    assert_eq!(result.metrics.tool_calls, 3);
    assert_eq!(result.metrics.failed_tool_calls, 1);
    assert_eq!(result.changed_files.len(), 1);
    assert_eq!(result.changed_files[0].path, "src/api/handler.rs");
    assert_eq!(result.artifacts.len(), 1);
    assert_eq!(result.validation.len(), 1);
    assert!(!result.validation[0].success);
    assert_eq!(result.findings.len(), 1);
    assert!(result.diagnostics[0].contains("shell"));
    assert_eq!(result.summary, "done");
}

#[test]
fn model_summary_is_compact_while_the_full_receipt_keeps_everything() {
    let mut result = ChildResult::new("child-9", "task-3", ChildStatus::Failed);
    result.summary = "attempted the migration".into();
    result.failure_reason = Some("compile error in migration.rs".into());
    result.changed_files = vec![ChangedFile {
        path: "migrations/2026_add_index.sql".into(),
        change: "created".into(),
    }];
    result.validation = vec![Validation {
        command: "cargo test".into(),
        success: false,
        detail: "3 failures".into(),
    }];
    result.findings = vec!["search: 4 matches".into()];
    result.diagnostics = vec!["shell: exit 101".into()];
    result.diff_stat = DiffStat {
        files: 1,
        insertions: Some(12),
        deletions: Some(2),
    };
    result.continuation_hint = Some("retry with the index name fixed".into());

    let summary = result.model_summary();
    assert!(summary.starts_with("Child child-9 failed:"));
    assert!(summary.contains("root cause: compile error in migration.rs"));
    assert!(summary.contains("relevant files: migrations/2026_add_index.sql"));
    assert!(summary.contains("validation: cargo test (failed)"));
    assert!(summary.contains("suggested next step: retry with the index name fixed"));
    assert!(summary.contains("child_result"));
    // The projection is a fraction of the record, and omits the raw detail.
    let full = serde_json::to_string(&result).unwrap();
    assert!(summary.len() < full.len());
    assert!(!summary.contains("cargo test\".into"));
    assert!(!summary.contains("insertions"));
}

#[test]
fn receipts_persist_and_restore_through_their_own_marker() {
    let mut receipts = BTreeMap::new();
    let mut first = ChildResult::new("child-a", "task-1", ChildStatus::Completed);
    first.summary = "did the first thing".into();
    store(&mut receipts, first);
    let mut second = ChildResult::new("child-b", "task-2", ChildStatus::Failed);
    second.failure_reason = Some("timed out on the second".into());
    store(&mut receipts, second);

    let marker = snapshot(&receipts).expect("non-empty index");
    let mut messages = vec![Message::user("hi"), marker.clone()];
    let restored = restore(&mut messages);
    assert_eq!(restored.len(), 2);
    assert_eq!(restored["child-a"].task_id, "task-1");
    assert_eq!(restored["child-b"].status, ChildStatus::Failed);
    // Markers never linger in conversation context.
    assert!(
        messages
            .iter()
            .all(|message| !message.content.starts_with(STATE_PREFIX))
    );
    assert_eq!(messages.len(), 1);

    // Round-tripping the marker twice is idempotent.
    let mut again = vec![snapshot(&restored).unwrap()];
    assert_eq!(restore(&mut again).len(), 2);
}

#[test]
fn receipts_are_bounded_deterministically() {
    let mut receipts = BTreeMap::new();
    for index in 0..MAX_RETAINED + 5 {
        store(
            &mut receipts,
            ChildResult::new(
                format!("child-{index:03}"),
                format!("task-{index}"),
                ChildStatus::Completed,
            ),
        );
    }
    assert_eq!(receipts.len(), MAX_RETAINED);
    assert!(!receipts.contains_key("child-000"));
    assert!(receipts.contains_key(&format!("child-{:03}", MAX_RETAINED + 4)));
}

#[test]
fn read_tool_serves_each_aspect_and_lists_when_no_id_is_given() {
    let mut receipts = BTreeMap::new();
    let mut result = ChildResult::new("child-x", "task-1", ChildStatus::Completed);
    result.summary = "summary text".into();
    result.diagnostics = vec!["boom".into()];
    result.artifacts = vec![Artifact {
        path: "out/report.md".into(),
        kind: "workspace-file".into(),
    }];
    result.diff_stat = DiffStat {
        files: 2,
        insertions: Some(9),
        deletions: Some(1),
    };
    result.validation = vec![Validation {
        command: "cargo fmt --check".into(),
        success: true,
        detail: String::new(),
    }];
    result.metrics.tool_calls = 5;
    store(&mut receipts, result);

    let full = apply_read(&receipts, &json!({"child_id": "child-x"})).unwrap();
    assert!(full.contains("summary text"));
    assert!(full.contains("out/report.md"));
    assert!(
        apply_read(
            &receipts,
            &json!({"child_id": "child-x", "aspect": "diagnostics"})
        )
        .unwrap()
        .contains("boom")
    );
    assert!(
        apply_read(&receipts, &json!({"child_id": "child-x", "aspect": "diff"}))
            .unwrap()
            .contains("\"files\": 2")
    );
    assert!(
        apply_read(
            &receipts,
            &json!({"child_id": "task-1", "aspect": "metrics"})
        )
        .unwrap()
        .contains("\"tool_calls\": 5")
    );
    let listing = apply_read(&receipts, &json!({})).unwrap();
    assert!(listing.contains("child-x") && listing.contains("task-1"));
    assert!(apply_read(&receipts, &json!({"child_id": "missing"})).is_err());
    assert!(
        apply_read(
            &receipts,
            &json!({"child_id": "child-x", "aspect": "nonsense"})
        )
        .is_err()
    );
}

#[test]
fn full_receipts_are_never_pushed_into_the_model_context() {
    let mut receipts = BTreeMap::new();
    let mut result = ChildResult::new("child-z", "task-1", ChildStatus::Completed);
    result.summary = "compact summary".into();
    result.diagnostics = vec!["very long diagnostic detail".into()];
    store(&mut receipts, result);
    let marker = snapshot(&receipts).unwrap();
    // The durable marker carries the full JSON...
    assert!(marker.content.contains("very long diagnostic detail"));
    // ...and the projection the controller forwards does not.
    let projection = receipts["child-z"].model_summary();
    assert!(!projection.contains("very long diagnostic detail"));
}
