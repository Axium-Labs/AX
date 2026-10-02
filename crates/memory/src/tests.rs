//! Store-level tests: schema, paging, JSONL indexing, compaction and resume.

use std::fs;

use rusqlite::Connection;
use serde_json::Value;
use uuid::Uuid;

use super::*;
use crate::events::append_event;

#[test]
fn session_messages_are_paginated_and_cascade_deleted() {
    let mut store = MemoryStore::open_in_memory().expect("store should open");
    let session = store
        .create_session("Runtime design")
        .expect("session should be created");
    let state = store
        .append_message(
            &session.id,
            NewMessage {
                role: MessageRole::System,
                kind: MessageKind::AgentState,
                content: "persistent skill".to_owned(),
                metadata: Value::Null,
            },
        )
        .expect("agent state should be stored");
    let first = store
        .append_message(&session.id, NewMessage::text(MessageRole::User, "first"))
        .expect("first message should be stored");
    let second = store
        .append_message(
            &session.id,
            NewMessage::text(MessageRole::Assistant, "second"),
        )
        .expect("second message should be stored");

    let latest = store
        .load_messages(&session.id, None, 1)
        .expect("latest page should load");
    assert_eq!(latest[0].id, second.id);
    let previous = store
        .load_messages(&session.id, Some(second.id), 10)
        .expect("previous page should load");
    assert!(previous.iter().any(|message| message.id == first.id));
    assert_eq!(
        store
            .session(&session.id)
            .expect("session query should work")
            .expect("session should exist")
            .message_count,
        3
    );

    store
        .save_context_summary(&session.id, 1, "summary", 1)
        .expect("session should compact");
    assert_eq!(
        store
            .session_summary(&session.id)
            .expect("summary query should work")
            .as_deref(),
        Some("summary")
    );
    assert_eq!(
        store
            .load_messages(&session.id, None, 10)
            .expect("compacted messages should load")
            .last()
            .expect("latest message should remain")
            .id,
        second.id
    );
    assert_eq!(
        store
            .load_agent_state_messages(&session.id)
            .expect("agent state should load")[0]
            .id,
        state.id
    );

    assert_eq!(
        store.load_messages(&session.id, None, 100).unwrap().len(),
        3
    );
    assert_eq!(store.load_context_messages(&session.id).unwrap().len(), 2);

    assert!(
        store
            .delete_session(&session.id)
            .expect("session should delete")
    );
    assert!(
        store
            .load_messages(&session.id, None, 10)
            .expect("message query should work")
            .is_empty()
    );
}

#[test]
fn effective_snapshot_resumes_only_its_session_and_keeps_raw_history() {
    let mut store = MemoryStore::open_in_memory().unwrap();
    let first = store.create_session("first").unwrap();
    let second = store.create_session("second").unwrap();
    store
        .append_message(
            &first.id,
            NewMessage::text(MessageRole::User, "original user text"),
        )
        .unwrap();
    store
        .append_message(
            &first.id,
            NewMessage::text(MessageRole::Tool, "very long raw test output"),
        )
        .unwrap();
    store
        .save_effective_context(&first.id, "GOAL: test", "[\"compressed context\"]")
        .unwrap();
    assert_eq!(
        store.effective_context(&first.id).unwrap().as_deref(),
        Some("[\"compressed context\"]")
    );
    assert_eq!(
        store.session_summary(&first.id).unwrap().as_deref(),
        Some("GOAL: test")
    );
    assert!(store.load_context_messages(&first.id).unwrap().is_empty());
    assert_eq!(store.load_messages(&first.id, None, 10).unwrap().len(), 2);
    assert!(store.effective_context(&second.id).unwrap().is_none());
    assert!(store.session_summary(&second.id).unwrap().is_none());
    store
        .append_message(
            &first.id,
            NewMessage::text(MessageRole::User, "after resume"),
        )
        .unwrap();
    assert_eq!(store.load_context_messages(&first.id).unwrap().len(), 1);
}

#[test]
fn long_term_memory_upserts_by_key_and_filters_by_category() {
    let store = MemoryStore::open_in_memory().expect("store should open");
    store
        .remember("language", "Rust", "preference")
        .expect("memory should be stored");
    store
        .remember("language", "Rust 2024", "preference")
        .expect("memory should be updated");
    store
        .remember("project", "ax runtime", "project")
        .expect("project should be stored");

    let preferences = store
        .list_long_term(Some("preference"), 20)
        .expect("memories should load");
    assert_eq!(preferences.len(), 1);
    assert_eq!(preferences[0].value, "Rust 2024");
    assert!(store.forget("language").expect("memory should delete"));
    assert!(
        store
            .recall("language")
            .expect("memory query should work")
            .is_none()
    );
}
#[test]
fn repeated_compaction_and_reopen_preserve_complete_history() {
    let path = std::env::temp_dir().join(format!("ax-history-test-{}.sqlite3", Uuid::new_v4()));
    let mut store = MemoryStore::open(&path).unwrap();
    let session = store.create_session("history").unwrap();
    for index in 0..20 {
        store
            .append_message(
                &session.id,
                NewMessage::text(MessageRole::User, format!("original {index}")),
            )
            .unwrap();
    }
    store
        .save_context_summary(&session.id, 4, "first summary", 16)
        .unwrap();
    store
        .save_context_summary(&session.id, 2, "second summary", 18)
        .unwrap();
    drop(store);
    let store = MemoryStore::open(&path).unwrap();
    let history = store.load_messages(&session.id, None, 100).unwrap();
    assert_eq!(history.len(), 20);
    assert_eq!(history[0].content, "original 0");
    assert_eq!(store.load_context_messages(&session.id).unwrap().len(), 2);
    assert_eq!(
        store.session_summary(&session.id).unwrap().as_deref(),
        Some("second summary")
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn migration_from_old_summary_schema_keeps_rows() {
    let mut store = MemoryStore::open_in_memory().unwrap();
    let session = store.create_session("legacy").unwrap();
    store.connection.execute("INSERT INTO session_summaries(session_id,content,compressed_message_count) VALUES (?1,'old summary',1)",[&session.id]).unwrap();
    store.connection.execute_batch("ALTER TABLE session_summaries DROP COLUMN through_message_id; PRAGMA user_version = 2;").unwrap();
    let connection =
        std::mem::replace(&mut store.connection, Connection::open_in_memory().unwrap());
    let events_dir = store.events_dir.clone();
    store.ephemeral_events = false;
    let migrated = MemoryStore::initialize(connection, events_dir, true).unwrap();
    assert_eq!(
        migrated.session_summary(&session.id).unwrap().as_deref(),
        Some("old summary")
    );
    assert!(
        migrated
            .load_context_messages(&session.id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn raw_events_live_in_jsonl_and_sqlite_keeps_only_the_index() {
    let root = std::env::temp_dir().join(format!("ax-hybrid-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let path = root.join("memory.sqlite3");
    let mut store = MemoryStore::open(&path).unwrap();
    let session = store.create_session("hybrid").unwrap();
    let state = store
        .append_message(
            &session.id,
            NewMessage {
                role: MessageRole::System,
                kind: MessageKind::AgentState,
                content: "active skill".into(),
                metadata: Value::Null,
            },
        )
        .unwrap();
    let event = store
        .append_message(
            &session.id,
            NewMessage::text(MessageRole::User, "raw private text"),
        )
        .unwrap();
    let indexed: (String, Option<i64>, Option<i64>) = store
        .connection
        .query_row(
            "SELECT content,event_offset,event_length FROM messages WHERE id=?1",
            [event.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(indexed.0.is_empty());
    assert!(indexed.1.is_some() && indexed.2.is_some());
    let log =
        fs::read_to_string(root.join("sessions").join(format!("{}.jsonl", session.id))).unwrap();
    assert_eq!(log.lines().count(), 2);
    assert!(log.contains("raw private text"));
    store
        .save_effective_context(&session.id, "short", "[]")
        .unwrap();
    drop(store);
    let store = MemoryStore::open(&path).unwrap();
    assert_eq!(
        store.load_messages(&session.id, None, 10).unwrap()[1].content,
        "raw private text"
    );
    assert_eq!(
        store.load_agent_state_messages(&session.id).unwrap()[0].id,
        state.id
    );
    assert_eq!(
        store.effective_context(&session.id).unwrap().as_deref(),
        Some("[]")
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_rows_and_unindexed_jsonl_events_recover_without_loss() {
    let mut store = MemoryStore::open_in_memory().unwrap();
    let session = store.create_session("legacy").unwrap();
    store.connection.execute(
            "INSERT INTO messages(session_id,role,kind,content,metadata) VALUES (?1,'user','message','legacy body','null')",
            [&session.id]).unwrap();
    let history = store.load_messages(&session.id, None, 10).unwrap();
    assert_eq!(history[0].content, "legacy body");
    let content: String = store
        .connection
        .query_row(
            "SELECT content FROM messages WHERE id=?1",
            [history[0].id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(content.is_empty());
    let orphan = StoredMessage {
        id: history[0].id + 1,
        session_id: session.id.clone(),
        role: MessageRole::Tool,
        kind: MessageKind::Message,
        content: "after crash".into(),
        metadata: Value::Null,
        created_at: 123,
    };
    append_event(&store.events_dir, &orphan).unwrap();
    assert_eq!(
        store.session(&session.id).unwrap().unwrap().message_count,
        2
    );
    assert_eq!(
        store.load_messages(&session.id, None, 10).unwrap()[1].content,
        "after crash"
    );
    let next = store
        .append_message(&session.id, NewMessage::text(MessageRole::User, "next"))
        .unwrap();
    assert!(next.id > orphan.id);
}
