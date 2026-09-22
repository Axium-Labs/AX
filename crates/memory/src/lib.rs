//! SQLite-backed session, history, and long-term memory.
//!
//! Opening the store and running its small migrations are explicit operations;
//! merely linking this crate performs no filesystem or database work.

mod scoped;
pub use scoped::{MemoryRecord, MemoryScope, extract_user_memories, retrieve};

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("memory metadata is invalid JSON: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid value in memory database: {0}")]
    InvalidValue(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    System,
}

impl MessageRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
            Self::System => "system",
        }
    }

    fn from_db(value: &str) -> Result<Self, MemoryError> {
        match value {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            "system" => Ok(Self::System),
            other => Err(MemoryError::InvalidValue(format!(
                "unknown message role: {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Message,
    ToolCall,
    McpCall,
    AgentState,
    Summary,
}

impl MessageKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::ToolCall => "tool_call",
            Self::McpCall => "mcp_call",
            Self::AgentState => "agent_state",
            Self::Summary => "summary",
        }
    }

    fn from_db(value: &str) -> Result<Self, MemoryError> {
        match value {
            "message" => Ok(Self::Message),
            "tool_call" => Ok(Self::ToolCall),
            "mcp_call" => Ok(Self::McpCall),
            "agent_state" => Ok(Self::AgentState),
            "summary" => Ok(Self::Summary),
            other => Err(MemoryError::InvalidValue(format!(
                "unknown message kind: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewMessage {
    pub role: MessageRole,
    pub kind: MessageKind,
    pub content: String,
    pub metadata: Value,
}

impl NewMessage {
    #[must_use]
    pub fn text(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            kind: MessageKind::Message,
            content: content.into(),
            metadata: Value::Null,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: String,
    pub role: MessageRole,
    pub kind: MessageKind,
    pub content: String,
    pub metadata: Value,
    pub created_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LongTermMemory {
    pub id: String,
    pub key: String,
    pub value: String,
    pub category: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct MemoryStore {
    connection: Connection,
}

impl MemoryStore {
    /// Opens or creates a memory database and applies lightweight migrations.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot open or migrate the database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    /// Creates an isolated in-memory store, primarily for tests and ephemeral runs.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` initialization fails.
    pub fn open_in_memory() -> Result<Self, MemoryError> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> Result<Self, MemoryError> {
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS sessions (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 updated_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS idx_sessions_updated_at
                 ON sessions(updated_at DESC);
             CREATE TABLE IF NOT EXISTS messages (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                 role TEXT NOT NULL,
                 kind TEXT NOT NULL DEFAULT 'message',
                 content TEXT NOT NULL,
                 metadata TEXT NOT NULL DEFAULT 'null',
                 created_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS idx_messages_session_id_id
                 ON messages(session_id, id DESC);
             CREATE TABLE IF NOT EXISTS long_term_memory (
                 id TEXT PRIMARY KEY,
                 key TEXT NOT NULL UNIQUE,
                 value TEXT NOT NULL,
                 category TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT (unixepoch()),
                 updated_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS idx_long_term_memory_category
                 ON long_term_memory(category, updated_at DESC);
             CREATE TABLE IF NOT EXISTS session_summaries (
                 session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                 content TEXT NOT NULL,
                 compressed_message_count INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL DEFAULT (unixepoch())
             );
             PRAGMA user_version = 2;",
        )?;
        let has_watermark: bool = connection.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session_summaries') WHERE name = 'through_message_id'", [], |row| row.get::<_, i64>(0)
        )? > 0;
        if !has_watermark {
            connection.execute("ALTER TABLE session_summaries ADD COLUMN through_message_id INTEGER NOT NULL DEFAULT 0", [])?;
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS scoped_memories (
            scope TEXT NOT NULL CHECK(scope IN ('global','project','session')), owner TEXT NOT NULL,
            key TEXT NOT NULL, value TEXT NOT NULL, source TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()), PRIMARY KEY(scope, owner, key));
            CREATE TABLE IF NOT EXISTS memory_migrations (category TEXT PRIMARY KEY, migrated_at INTEGER NOT NULL DEFAULT (unixepoch()));
            PRAGMA user_version = 4;",
        )?;
        Ok(Self { connection })
    }

    /// Creates a new session without loading any previous session history.
    ///
    /// # Errors
    ///
    /// Returns an error when the session cannot be inserted or read back.
    pub fn create_session(&self, title: impl AsRef<str>) -> Result<Session, MemoryError> {
        let id = Uuid::new_v4().to_string();
        let title = normalized_title(title.as_ref());
        self.connection.execute(
            "INSERT INTO sessions (id, title) VALUES (?1, ?2)",
            params![id, title],
        )?;
        self.session(&id)?.ok_or_else(|| {
            MemoryError::InvalidValue("newly created session was not found".to_owned())
        })
    }

    /// Retrieves one session and its message count.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails or persisted values are invalid.
    pub fn session(&self, id: &str) -> Result<Option<Session>, MemoryError> {
        self.connection
            .query_row(
                "SELECT s.id, s.title, s.created_at, s.updated_at, COUNT(m.id)
                 FROM sessions s
                 LEFT JOIN messages m ON m.session_id = s.id
                 WHERE s.id = ?1
                 GROUP BY s.id",
                [id],
                map_session,
            )
            .optional()
            .map_err(MemoryError::from)
    }

    /// Returns a recent-session page ordered by most recently updated first.
    ///
    /// # Errors
    ///
    /// Returns an error when the database query fails.
    pub fn list_sessions(&self, limit: u32, offset: u32) -> Result<Vec<Session>, MemoryError> {
        let mut statement = self.connection.prepare(
            "SELECT s.id, s.title, s.created_at, s.updated_at, COUNT(m.id)
             FROM sessions s
             LEFT JOIN messages m ON m.session_id = s.id
             GROUP BY s.id
             ORDER BY s.updated_at DESC, s.id DESC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(params![limit, offset], map_session)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::from)
    }

    /// Changes a session title.
    ///
    /// # Errors
    ///
    /// Returns an error when the update fails.
    pub fn rename_session(&self, id: &str, title: impl AsRef<str>) -> Result<bool, MemoryError> {
        let changed = self.connection.execute(
            "UPDATE sessions SET title = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, normalized_title(title.as_ref())],
        )?;
        Ok(changed == 1)
    }

    /// Deletes a session and its messages through an `SQLite` foreign-key cascade.
    ///
    /// # Errors
    ///
    /// Returns an error when the deletion fails.
    pub fn delete_session(&self, id: &str) -> Result<bool, MemoryError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM scoped_memories WHERE scope='session' AND owner=?1",
            [id],
        )?;
        let deleted = transaction.execute("DELETE FROM sessions WHERE id=?1", [id])? == 1;
        transaction.commit()?;
        Ok(deleted)
    }

    /// Appends one message and marks its session as recently updated.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization or the transaction fails.
    pub fn append_message(
        &mut self,
        session_id: &str,
        message: NewMessage,
    ) -> Result<StoredMessage, MemoryError> {
        let NewMessage {
            role,
            kind,
            content,
            metadata,
        } = message;
        let metadata = serde_json::to_string(&metadata)?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO messages (session_id, role, kind, content, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role.as_str(), kind.as_str(), content, metadata],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.execute(
            "UPDATE sessions SET updated_at = unixepoch() WHERE id = ?1",
            [session_id],
        )?;
        transaction.commit()?;
        self.message(id)?.ok_or_else(|| {
            MemoryError::InvalidValue("newly inserted message was not found".to_owned())
        })
    }

    /// Loads one page of a session, returning messages in chronological order.
    /// `before_id` provides stable backward pagination without loading full history.
    ///
    /// # Errors
    ///
    /// Returns an error when the query or metadata decoding fails.
    pub fn load_messages(
        &self,
        session_id: &str,
        before_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<StoredMessage>, MemoryError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, role, kind, content, metadata, created_at
             FROM (
                 SELECT id, session_id, role, kind, content, metadata, created_at
                 FROM messages
                 WHERE session_id = ?1 AND (?2 IS NULL OR id < ?2)
                 ORDER BY id DESC
                 LIMIT ?3
             )
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map(params![session_id, before_id, limit], map_raw_message)?;
        rows.map(|row| decode_message(row?)).collect()
    }

    /// Loads the small set of persistent system/agent-state messages for a session.
    /// These are kept verbatim across summary compaction (for example, active Skill
    /// instructions) and are fetched separately from the bounded recent-message page.
    ///
    /// # Errors
    ///
    /// Returns an error when the query or metadata decoding fails.
    pub fn load_agent_state_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<StoredMessage>, MemoryError> {
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, role, kind, content, metadata, created_at
             FROM messages
             WHERE session_id = ?1 AND kind = 'agent_state'
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map([session_id], map_raw_message)?;
        rows.map(|row| decode_message(row?)).collect()
    }

    /// Returns the current compressed summary for a session, if one exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails.
    pub fn session_summary(&self, session_id: &str) -> Result<Option<String>, MemoryError> {
        self.connection
            .query_row(
                "SELECT content FROM session_summaries WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(MemoryError::from)
    }

    /// Saves a context summary and coverage watermark without deleting any history.
    ///
    /// # Errors
    ///
    /// Returns an error when the compaction transaction fails.
    pub fn save_context_summary(
        &mut self,
        session_id: &str,
        keep_latest: u32,
        summary: &str,
        compressed_message_count: usize,
    ) -> Result<(), MemoryError> {
        let transaction = self.connection.transaction()?;
        let through: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE session_id = ?1 AND id NOT IN
             (SELECT id FROM messages WHERE session_id = ?1 ORDER BY id DESC LIMIT ?2)",
            params![session_id, keep_latest],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO session_summaries (session_id, content, compressed_message_count, updated_at, through_message_id)
             VALUES (?1, ?2, ?3, unixepoch(), ?4)
             ON CONFLICT(session_id) DO UPDATE SET content = excluded.content,
             compressed_message_count = excluded.compressed_message_count, updated_at = excluded.updated_at,
             through_message_id = MAX(session_summaries.through_message_id, excluded.through_message_id)",
            params![session_id, summary, compressed_message_count, through],
        )?;
        transaction.commit()?;
        Ok(())
    }

    ///
    /// # Errors
    /// Returns an error if the database cannot be queried or updated.
    pub fn legacy_scope_migrated(&self, category: &str) -> Result<bool, MemoryError> {
        Ok(self
            .connection
            .query_row(
                "SELECT 1 FROM memory_migrations WHERE category=?1",
                [category],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_some())
    }
    ///
    /// # Errors
    /// Returns an error if the database cannot be queried or updated.
    pub fn mark_legacy_scope_migrated(&self, category: &str) -> Result<(), MemoryError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO memory_migrations(category) VALUES (?1)",
            [category],
        )?;
        Ok(())
    }

    /// Load uncompacted context plus persistent agent state. UI history uses `load_messages`.
    ///
    /// # Errors
    /// Returns an error if the database cannot be queried or updated.
    pub fn load_context_messages(
        &self,
        session_id: &str,
    ) -> Result<Vec<StoredMessage>, MemoryError> {
        let all = self.load_messages(session_id, None, u32::MAX)?;
        let through: i64 = self
            .connection
            .query_row(
                "SELECT through_message_id FROM session_summaries WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok(all
            .into_iter()
            .filter(|message| message.id > through || message.kind == MessageKind::AgentState)
            .collect())
    }

    fn message(&self, id: i64) -> Result<Option<StoredMessage>, MemoryError> {
        let raw = self
            .connection
            .query_row(
                "SELECT id, session_id, role, kind, content, metadata, created_at
                 FROM messages WHERE id = ?1",
                [id],
                map_raw_message,
            )
            .optional()?;
        raw.map(decode_message).transpose()
    }

    /// Creates or replaces one long-term memory value by its stable key.
    ///
    /// # Errors
    ///
    /// Returns an error when the upsert or follow-up query fails.
    pub fn remember(
        &self,
        key: impl AsRef<str>,
        value: impl AsRef<str>,
        category: impl AsRef<str>,
    ) -> Result<LongTermMemory, MemoryError> {
        let id = Uuid::new_v4().to_string();
        self.connection.execute(
            "INSERT INTO long_term_memory (id, key, value, category)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET
                 value = excluded.value,
                 category = excluded.category,
                 updated_at = unixepoch()",
            params![id, key.as_ref(), value.as_ref(), category.as_ref()],
        )?;
        self.recall(key.as_ref())?.ok_or_else(|| {
            MemoryError::InvalidValue("upserted long-term memory was not found".to_owned())
        })
    }

    /// Retrieves one long-term memory value by key.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails.
    pub fn recall(&self, key: &str) -> Result<Option<LongTermMemory>, MemoryError> {
        self.connection
            .query_row(
                "SELECT id, key, value, category, created_at, updated_at
                 FROM long_term_memory WHERE key = ?1",
                [key],
                map_long_term_memory,
            )
            .optional()
            .map_err(MemoryError::from)
    }

    /// Lists long-term memories, optionally restricted to one category.
    ///
    /// # Errors
    ///
    /// Returns an error when the query fails.
    pub fn list_long_term(
        &self,
        category: Option<&str>,
        limit: u32,
    ) -> Result<Vec<LongTermMemory>, MemoryError> {
        let mut statement = self.connection.prepare(
            "SELECT id, key, value, category, created_at, updated_at
             FROM long_term_memory
             WHERE (?1 IS NULL OR category = ?1)
             ORDER BY updated_at DESC, key ASC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![category, limit], map_long_term_memory)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MemoryError::from)
    }

    /// Removes one long-term memory value.
    ///
    /// # Errors
    ///
    /// Returns an error when the deletion fails.
    pub fn forget(&self, key: &str) -> Result<bool, MemoryError> {
        Ok(self
            .connection
            .execute("DELETE FROM long_term_memory WHERE key = ?1", [key])?
            == 1)
    }
}

fn normalized_title(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        "Untitled session".to_owned()
    } else {
        title.chars().take(120).collect()
    }
}

fn map_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
        message_count: row.get(4)?,
    })
}

type RawMessage = (i64, String, String, String, String, String, i64);

fn map_raw_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMessage> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn decode_message(raw: RawMessage) -> Result<StoredMessage, MemoryError> {
    Ok(StoredMessage {
        id: raw.0,
        session_id: raw.1,
        role: MessageRole::from_db(&raw.2)?,
        kind: MessageKind::from_db(&raw.3)?,
        content: raw.4,
        metadata: serde_json::from_str(&raw.5)?,
        created_at: raw.6,
    })
}

fn map_long_term_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<LongTermMemory> {
    Ok(LongTermMemory {
        id: row.get(0)?,
        key: row.get(1)?,
        value: row.get(2)?,
        category: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let store = MemoryStore::open_in_memory().unwrap();
        let session = store.create_session("legacy").unwrap();
        store.connection.execute("INSERT INTO session_summaries(session_id,content,compressed_message_count) VALUES (?1,'old summary',1)",[&session.id]).unwrap();
        store.connection.execute_batch("ALTER TABLE session_summaries DROP COLUMN through_message_id; PRAGMA user_version = 2;").unwrap();
        let migrated = MemoryStore::initialize(store.connection).unwrap();
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
}
