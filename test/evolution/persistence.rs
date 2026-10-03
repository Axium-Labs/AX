use super::*;

fn reopen(f: &Fixture) -> Engine {
    Engine::open(
        f.engine.root.clone(),
        f.engine.database.clone(),
        f.engine.project.clone(),
    )
    .unwrap()
}
fn ledger(f: &Fixture) -> serde_json::Value {
    serde_json::from_slice(&fs::read(f.engine.root.join("ledger.json")).unwrap()).unwrap()
}
fn stream(f: &Fixture) -> String {
    fs::read_to_string(f.engine.root.join("experiences.jsonl")).unwrap()
}
fn old_ledger(f: &Fixture, experiences: &[Experience], pending: usize) {
    fs::write(
        f.engine.root.join("ledger.json"),
        serde_json::to_vec(&serde_json::json!({
            "skills": f.engine.ledger.skills, "experiences": experiences,
            "pending": pending, "last_analysis": 123, "epoch": 7
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn legacy_migration_is_idempotent_and_jsonl_wins() {
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    let original = stream(&f);
    let mut copy = experience(0);
    copy.task = "Different legacy copy".into();
    old_ledger(&f, &[copy, experience(1)], 2);
    let engine = reopen(&f);
    assert_eq!(engine.recent[0].task, experience(0).task);
    assert_eq!(engine.ledger.pending, 2);
    assert_eq!(engine.ledger.epoch, 7);
    assert_eq!(engine.ledger.last_analysis, 123);
    assert!(stream(&f).starts_with(&original));
    assert_eq!(stream(&f).lines().count(), 2);
    assert_eq!(ledger(&f)["version"], 2);
    assert!(ledger(&f).get("experiences").is_none());
    let migrated = stream(&f);
    reopen(&f);
    assert_eq!(stream(&f), migrated);
}

#[test]
fn legacy_pending_suffix_and_orphan_append_are_preserved() {
    let mut f = Fixture::new();
    for i in 0..5 {
        f.engine.record(experience(i)).unwrap();
    }
    old_ledger(&f, &[experience(2), experience(3)], 2);
    let mut engine = reopen(&f);
    let (input, cursor) = engine.prepare_analysis(100_000).unwrap();
    let data: serde_json::Value = serde_json::from_str(&input).unwrap();
    assert!(
        data["experiences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"] == "e-2")
    );
    assert_eq!(engine.ledger.pending, 3); // e-2, e-3 and JSONL-only e-4
    engine.complete_analysis(cursor).unwrap();
    assert_eq!(reopen(&f).ledger.pending, 0);
}

#[test]
fn bounded_batches_restart_and_never_skip_backlog() {
    let mut f = Fixture::new();
    f.engine.config.max_experiences = 2;
    f.engine.config.batch_size = 2;
    fs::write(
        f.engine.root.join("config.json"),
        serde_json::to_vec(&f.engine.config).unwrap(),
    )
    .unwrap();
    for i in 0..5 {
        f.engine.record(experience(i)).unwrap();
    }
    let mut engine = reopen(&f);
    let mut consumed = Vec::new();
    while engine.ledger.pending > 0 {
        let before = engine.ledger.processed_cursor;
        let (input, cursor) = engine.prepare_analysis(100_000).unwrap();
        assert_eq!(engine.ledger.processed_cursor, before);
        let data: serde_json::Value = serde_json::from_str(&input).unwrap();
        for e in data["experiences"].as_array().unwrap() {
            let id = e["id"].as_str().unwrap().to_owned();
            if !consumed.contains(&id) {
                consumed.push(id);
            }
        }
        assert!(engine.recent.len() <= 2);
        engine.complete_analysis(cursor).unwrap();
        engine = reopen(&f);
    }
    assert_eq!(
        consumed,
        (0..5).map(|i| format!("e-{i}")).collect::<Vec<_>>()
    );
    assert_eq!(engine.ledger.processed_cursor, stream(&f).len() as u64);
}

#[test]
fn duplicate_outside_recent_window_and_after_restart_is_not_appended() {
    let mut f = Fixture::new();
    f.engine.config.max_experiences = 2;
    for i in 0..5 {
        f.engine.record(experience(i)).unwrap();
    }
    let original = stream(&f);
    f.engine.record(experience(0)).unwrap();
    let mut engine = reopen(&f);
    engine.record(experience(0)).unwrap();
    assert_eq!(stream(&f), original);
    assert_eq!(engine.ledger.pending, 5);
    assert!(ledger(&f).get("recent").is_none());
    assert!(ledger(&f).get("experiences").is_none());
}

#[test]
fn incremental_reader_does_not_visit_old_bytes() {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    let cursor = stream(&f).len() as u64;
    let path = f.engine.root.join("experiences.jsonl");
    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"!").unwrap(); // old bytes would fail JSON decoding if rescanned
    crate::storage::append_experience(&path, &experience(1)).unwrap();
    let rows = crate::storage::read_experiences(&path, cursor)
        .unwrap()
        .collect::<anyhow::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.id, "e-1");
    f.engine.refresh().unwrap(); // cached Engine also seeks past old bytes
    assert_eq!(f.engine.ledger.pending, 2);
}

#[test]
fn jsonl_append_survives_failed_ledger_save_without_double_usage() {
    let mut f = Fixture::new();
    let ids = f.stable();
    f.engine
        .apply(create("release-check", ids.clone()), 100_000)
        .unwrap();
    f.engine
        .apply(
            Action::Promote {
                name: "release-check".into(),
                evidence: ids,
            },
            100_000,
        )
        .unwrap();
    let path = f.engine.root.join("ledger.json");
    let backup = f.engine.root.join("ledger.backup");
    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();
    let mut e = experience(6);
    e.skills_used.push("release-check".into());
    crate::storage::append_experience(&f.engine.root.join("experiences.jsonl"), &e).unwrap();
    assert!(f.engine.record(e.clone()).is_err());
    assert_eq!(f.engine.ledger.skills["release-check"].use_count, 0);
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    let mut engine = reopen(&f);
    assert_eq!(engine.ledger.skills["release-check"].use_count, 1);
    let original = stream(&f);
    engine.record(e).unwrap();
    assert_eq!(stream(&f), original);
    assert_eq!(reopen(&f).ledger.skills["release-check"].use_count, 1);
}

#[test]
fn failed_cursor_checkpoint_keeps_durable_and_runtime_progress() {
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    let (_, cursor) = f.engine.prepare_analysis(100_000).unwrap();
    let path = f.engine.root.join("ledger.json");
    let backup = f.engine.root.join("ledger.backup");
    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(f.engine.complete_analysis(cursor).is_err());
    assert_eq!(f.engine.ledger.processed_cursor, 0);
    assert_eq!(f.engine.ledger.pending, 1);
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    assert_eq!(reopen(&f).ledger.processed_cursor, 0);
}

#[test]
fn torn_tail_and_truncated_stream_fail_without_modifying_user_data() {
    use std::io::Write;
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    let path = f.engine.root.join("experiences.jsonl");
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"id\":")
        .unwrap();
    let original = stream(&f);
    assert!(
        Engine::open(
            f.engine.root.clone(),
            f.engine.database.clone(),
            f.engine.project.clone()
        )
        .is_err()
    );
    assert!(f.engine.record(experience(1)).is_err());
    assert_eq!(stream(&f), original);
    fs::write(&path, b"").unwrap();
    assert!(
        Engine::open(
            f.engine.root.clone(),
            f.engine.database.clone(),
            f.engine.project.clone()
        )
        .is_err()
    );
    assert_eq!(ledger(&f)["processed_cursor"], 0);
}

#[test]
fn context_fitting_preserves_first_pending_and_only_commits_selected_prefix() {
    let mut f = Fixture::new();
    for i in 0..4 {
        let mut e = experience(i);
        e.task = "A reusable workflow ".repeat(100);
        f.engine.record(e).unwrap();
    }
    let (input, cursor) = f.engine.prepare_analysis(6000).unwrap();
    let data: serde_json::Value = serde_json::from_str(&input).unwrap();
    let selected = data["experiences"].as_array().unwrap();
    assert_eq!(selected[0]["id"], "e-0");
    assert!(selected.len() < 4);
    f.engine.complete_analysis(cursor).unwrap();
    assert_eq!(f.engine.ledger.pending, 4 - selected.len());
}

#[tokio::test]
async fn audit_failure_keeps_cursor_and_restart_consumes_without_new_records() {
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    let audit = f.engine.root.join("decisions.jsonl");
    fs::create_dir(&audit).unwrap();
    let analyzer = std::sync::Arc::new(Analyzer {
        calls: std::sync::atomic::AtomicUsize::new(0),
        response: "[{\"action\":\"IGNORE\"}]".into(),
        delay: false,
    });
    start(
        f.engine.root.clone(),
        f.engine.database.clone(),
        f.engine.project.clone(),
        analyzer.clone(),
        vec![],
    )
    .finish()
    .await;
    let mut engine = reopen(&f);
    assert_eq!(engine.ledger.processed_cursor, 0);
    assert_eq!(engine.ledger.pending, 1);
    fs::remove_dir(&audit).unwrap();
    engine.ledger.last_analysis = 0; // make the persisted cooldown eligible
    engine.save().unwrap();
    start(
        f.engine.root.clone(),
        f.engine.database.clone(),
        f.engine.project.clone(),
        analyzer.clone(),
        vec![],
    )
    .finish()
    .await;
    let engine = reopen(&f);
    assert_eq!(engine.ledger.pending, 0);
    assert_eq!(engine.ledger.processed_cursor, stream(&f).len() as u64);
    assert_eq!(analyzer.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(stream(&f).lines().count(), 1);
}

#[test]
fn legacy_migration_preserves_skill_usage_and_recovers_orphan_usage() {
    let mut f = Fixture::new();
    let ids = f.stable();
    f.engine
        .apply(create("release-check", ids.clone()), 100_000)
        .unwrap();
    f.engine
        .apply(
            Action::Promote {
                name: "release-check".into(),
                evidence: ids,
            },
            100_000,
        )
        .unwrap();
    let mut e = experience(6);
    e.skills_used.push("release-check".into());
    f.engine.record(e.clone()).unwrap();
    old_ledger(&f, &[e], 1);
    let mut orphan = experience(7);
    orphan.skills_used.push("release-check".into());
    crate::storage::append_experience(&f.engine.root.join("experiences.jsonl"), &orphan).unwrap();
    let engine = reopen(&f);
    assert_eq!(engine.ledger.skills["release-check"].use_count, 2);
    assert_eq!(engine.ledger.pending, 2);
    assert_eq!(reopen(&f).ledger.skills["release-check"].use_count, 2);
}

#[tokio::test]
async fn memory_storage_failure_keeps_pending_cursor() {
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    f.engine.record(experience(1)).unwrap();
    fs::create_dir(&f.engine.database).unwrap();
    let analyzer = std::sync::Arc::new(Analyzer {
        calls: std::sync::atomic::AtomicUsize::new(0),
        delay: false,
        response: serde_json::to_string(&vec![Action::Memory {
            key: "build.preference".into(),
            value: "Verify release artifacts".into(),
            evidence: vec!["e-0".into(), "e-1".into()],
            quote: "Build release artifacts".into(),
            confidence: 1.0,
        }])
        .unwrap(),
    });
    start(
        f.engine.root.clone(),
        f.engine.database.clone(),
        f.engine.project.clone(),
        analyzer,
        vec![],
    )
    .finish()
    .await;
    let engine = reopen(&f);
    assert_eq!(engine.ledger.pending, 2);
    assert_eq!(engine.ledger.processed_cursor, 0);
}

#[test]
fn legacy_ledger_only_consumed_experiences_migrate_without_reanalysis() {
    let f = Fixture::new();
    old_ledger(&f, &[experience(0), experience(1)], 0);
    let engine = reopen(&f);
    assert_eq!(stream(&f).lines().count(), 2);
    assert_eq!(engine.ledger.pending, 0);
    assert_eq!(engine.ledger.processed_cursor, stream(&f).len() as u64);
}

#[test]
fn invalid_cursor_and_future_schema_do_not_overwrite_ledger() {
    let mut f = Fixture::new();
    f.engine.record(experience(0)).unwrap();
    for (field, value) in [("processed_cursor", 1), ("version", 99)] {
        let mut data = ledger(&f);
        data[field] = value.into();
        let bytes = serde_json::to_vec(&data).unwrap();
        fs::write(f.engine.root.join("ledger.json"), &bytes).unwrap();
        assert!(
            Engine::open(
                f.engine.root.clone(),
                f.engine.database.clone(),
                f.engine.project.clone()
            )
            .is_err()
        );
        assert_eq!(fs::read(f.engine.root.join("ledger.json")).unwrap(), bytes);
        f.engine.save().unwrap();
    }
}

#[test]
fn interrupted_migration_keeps_original_pending_boundary() {
    let f = Fixture::new();
    // Simulate an old checkpoint missing both an orphan append and a
    // legacy-only pending record. Migration had checkpointed its offsets
    // and appended the missing record, but had not published version 2.
    crate::storage::append_experience(&f.engine.root.join("experiences.jsonl"), &experience(1))
        .unwrap();
    let original_end = stream(&f).len() as u64;
    old_ledger(&f, &[experience(0)], 1);
    let mut data = ledger(&f);
    data["experience_migration"] = serde_json::json!({
        "original_end": original_end, "known_end": 0,
        "processed": 0, "consume_all": false
    });
    fs::write(
        f.engine.root.join("ledger.json"),
        serde_json::to_vec(&data).unwrap(),
    )
    .unwrap();
    crate::storage::append_experience(&f.engine.root.join("experiences.jsonl"), &experience(0))
        .unwrap();
    let original = stream(&f);
    let engine = reopen(&f);
    assert_eq!(engine.ledger.processed_cursor, 0);
    assert_eq!(engine.ledger.pending, 2);
    assert_eq!(stream(&f), original);
    assert!(ledger(&f).get("experience_migration").is_none());
    assert!(ledger(&f).get("experiences").is_none());
}

#[test]
fn missing_already_consumed_legacy_copy_does_not_skip_existing_pending_work() {
    let f = Fixture::new();
    for i in 1..3 {
        crate::storage::append_experience(&f.engine.root.join("experiences.jsonl"), &experience(i))
            .unwrap();
    }
    old_ledger(&f, &[experience(0), experience(1), experience(2)], 2);
    let mut engine = reopen(&f);
    let (input, _) = engine.prepare_analysis(100_000).unwrap();
    let data: serde_json::Value = serde_json::from_str(&input).unwrap();
    assert_eq!(data["experiences"][0]["id"], "e-1");
    assert_eq!(engine.ledger.processed_cursor, 0);
    // The consumed missing copy was appended later, so conservative replay of
    // that extra record is allowed; existing pending records must not be lost.
    assert_eq!(engine.ledger.pending, 3);
}
