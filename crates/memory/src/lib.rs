//! `SQLite` metadata and memory with per-session JSONL event history.
//!
//! Opening the store and running its small migrations are explicit operations;
//! merely linking this crate performs no filesystem or database work.

pub mod backup;
mod scoped;
pub use scoped::{MemoryRecord, MemoryScope, extract_user_memories, retrieve, validate_fact};

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("session event I/O error: {0}")]
    Io(#[from] std::io::Error),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    events_dir: PathBuf,
    ephemeral_events: bool,
}

impl MemoryStore {
    /// Opens or creates a memory database and applies lightweight migrations.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot open or migrate the database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        let path = path.as_ref();
        let connection = Connection::open(path)?;
        let events_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sessions");
        Self::initialize(connection, events_dir, false)
    }

    /// Creates an isolated in-memory store, primarily for tests and ephemeral runs.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` initialization fails.
    pub fn open_in_memory() -> Result<Self, MemoryError> {
        let events_dir = std::env::temp_dir().join(format!("ax-session-events-{}", Uuid::new_v4()));
        Self::initialize(Connection::open_in_memory()?, events_dir, true)
    }

    fn initialize(
        connection: Connection,
        events_dir: PathBuf,
        ephemeral_events: bool,
    ) -> Result<Self, MemoryError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
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
        let has_effective: bool = connection.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('session_summaries') WHERE name = 'effective_context'", [], |row| row.get::<_, i64>(0)
        )? > 0;
        if !has_effective {
            connection.execute(
                "ALTER TABLE session_summaries ADD COLUMN effective_context TEXT",
                [],
            )?;
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS scoped_memories (
            scope TEXT NOT NULL CHECK(scope IN ('global','project','session')), owner TEXT NOT NULL,
            key TEXT NOT NULL, value TEXT NOT NULL, source TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()), PRIMARY KEY(scope, owner, key));
            CREATE TABLE IF NOT EXISTS memory_migrations (category TEXT PRIMARY KEY, migrated_at INTEGER NOT NULL DEFAULT (unixepoch()));
            PRAGMA user_version = 4;",
        )?;
        let has_always: i64 = connection.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('scoped_memories') WHERE name='always_include'",
            [],
            |row| row.get(0),
        )?;
        if has_always == 0 {
            connection.execute(
                "ALTER TABLE scoped_memories ADD COLUMN always_include INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        connection.execute_batch("CREATE TABLE IF NOT EXISTS project_memory_migrations(owner TEXT PRIMARY KEY, project_id TEXT NOT NULL); PRAGMA user_version=5;")?;
        for column in ["event_offset", "event_length"] {
            let exists: i64 = connection.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name=?1",
                [column],
                |row| row.get(0),
            )?;
            if exists == 0 {
                connection.execute(
                    &format!("ALTER TABLE messages ADD COLUMN {column} INTEGER"),
                    [],
                )?;
            }
        }
        connection.execute_batch("CREATE TABLE IF NOT EXISTS agent_states (
            message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            content TEXT NOT NULL, metadata TEXT NOT NULL, created_at INTEGER NOT NULL
        ); CREATE INDEX IF NOT EXISTS idx_agent_states_session ON agent_states(session_id, message_id);
        PRAGMA user_version=6;")?;
        Ok(Self {
            connection,
            events_dir,
            ephemeral_events,
        })
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
        self.sync_session(id)?;
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
        let sessions = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        sessions
            .into_iter()
            .map(|session: Session| {
                self.session(&session.id)?
                    .ok_or_else(|| MemoryError::InvalidValue("listed session disappeared".into()))
            })
            .collect()
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
        if deleted {
            match fs::remove_file(self.event_path(id)?) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        sync_session_locked(&transaction, &self.events_dir, session_id)?;
        transaction.execute(
            "INSERT INTO messages (session_id, role, kind, content, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, role.as_str(), kind.as_str(), "", "null"],
        )?;
        let id = transaction.last_insert_rowid();
        let created_at: i64 =
            transaction.query_row("SELECT created_at FROM messages WHERE id=?1", [id], |row| {
                row.get(0)
            })?;
        let stored = StoredMessage {
            id,
            session_id: session_id.to_owned(),
            role,
            kind,
            content,
            metadata,
            created_at,
        };
        let (offset, length) = append_event(&self.events_dir, &stored)?;
        transaction.execute(
            "UPDATE messages SET event_offset=?2,event_length=?3 WHERE id=?1",
            params![id, offset, length],
        )?;
        if kind == MessageKind::AgentState {
            transaction.execute("INSERT INTO agent_states(message_id,session_id,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
                params![id, session_id, stored.content, serde_json::to_string(&stored.metadata)?, created_at])?;
        }
        transaction.execute(
            "UPDATE sessions SET updated_at = unixepoch() WHERE id = ?1",
            [session_id],
        )?;
        transaction.commit()?;
        Ok(stored)
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
        self.sync_session(session_id)?;
        let mut statement = self.connection.prepare(
            "SELECT id, session_id, role, kind, content, metadata, created_at, event_offset, event_length
             FROM (
                 SELECT id, session_id, role, kind, content, metadata, created_at, event_offset, event_length
                 FROM messages
                 WHERE session_id = ?1 AND (?2 IS NULL OR id < ?2)
                 ORDER BY id DESC
                 LIMIT ?3
             )
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map(params![session_id, before_id, limit], map_raw_message)?;
        rows.map(|row| self.decode_indexed(row?)).collect()
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
        self.sync_session(session_id)?;
        let mut statement = self.connection.prepare(
            "SELECT a.message_id,a.session_id,m.role,'agent_state',a.content,a.metadata,a.created_at,NULL,NULL
             FROM agent_states a JOIN messages m ON m.id=a.message_id
             WHERE a.session_id=?1 ORDER BY a.message_id ASC",
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

    /// Restores the exact effective prefix saved at the last compression.
    ///
    /// # Errors
    /// Returns an error if the session row cannot be read.
    pub fn effective_context(&self, session_id: &str) -> Result<Option<String>, MemoryError> {
        self.connection
            .query_row(
                "SELECT effective_context FROM session_summaries WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(MemoryError::from)
    }

    /// Saves a session-local effective snapshot at the current raw-message watermark.
    ///
    /// # Errors
    /// Returns an error if the transaction cannot be committed.
    pub fn save_effective_context(
        &mut self,
        session_id: &str,
        summary: &str,
        effective_json: &str,
    ) -> Result<(), MemoryError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        sync_session_locked(&transaction, &self.events_dir, session_id)?;
        let through: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM messages WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO session_summaries (session_id, content, compressed_message_count, updated_at, through_message_id, effective_context)
             VALUES (?1, ?2, 0, unixepoch(), ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET content=excluded.content,
             updated_at=excluded.updated_at, through_message_id=excluded.through_message_id,
             effective_context=excluded.effective_context",
            params![session_id, summary, through, effective_json],
        )?;
        transaction.commit()?;
        Ok(())
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
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        sync_session_locked(&transaction, &self.events_dir, session_id)?;
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
             through_message_id = MAX(session_summaries.through_message_id, excluded.through_message_id),
             effective_context = NULL",
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
        self.load_context_page(session_id, None, u32::MAX)
    }

    /// Read a bounded, chronological page after the effective-context watermark.
    ///
    /// # Errors
    /// Returns an error if the database cannot be queried or decoded.
    pub fn load_context_page(
        &self,
        session_id: &str,
        before_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<StoredMessage>, MemoryError> {
        self.sync_session(session_id)?;
        let through: i64 = self
            .connection
            .query_row(
                "SELECT through_message_id FROM session_summaries WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let has_snapshot = self.effective_context(session_id)?.is_some();
        let mut statement = self.connection.prepare(
            "SELECT id,session_id,role,kind,content,metadata,created_at,event_offset,event_length FROM (
             SELECT id,session_id,role,kind,content,metadata,created_at,event_offset,event_length FROM messages
             WHERE session_id=?1 AND (id>?2 OR (kind='agent_state' AND ?3=0))
             AND (?4 IS NULL OR id<?4) ORDER BY id DESC LIMIT ?5) ORDER BY id ASC",
        )?;
        let rows = statement.query_map(
            params![session_id, through, has_snapshot, before_id, limit],
            map_raw_message,
        )?;
        rows.map(|row| self.decode_indexed(row?)).collect()
    }

    fn event_path(&self, session_id: &str) -> Result<PathBuf, MemoryError> {
        event_path(&self.events_dir, session_id)
    }

    fn decode_indexed(&self, raw: RawMessage) -> Result<StoredMessage, MemoryError> {
        if let (Some(offset), Some(length)) = (raw.7, raw.8) {
            let mut file = File::open(self.event_path(&raw.1)?)?;
            file.seek(SeekFrom::Start(offset.try_into().map_err(|_| {
                MemoryError::InvalidValue("negative event offset".into())
            })?))?;
            let mut bytes = vec![
                0;
                usize::try_from(length).map_err(|_| MemoryError::InvalidValue(
                    "invalid event length".into()
                ))?
            ];
            file.read_exact(&mut bytes)?;
            let event: StoredMessage = serde_json::from_slice(&bytes)?;
            if event.id != raw.0 || event.session_id != raw.1 {
                return Err(MemoryError::InvalidValue(
                    "event index does not match JSONL".into(),
                ));
            }
            Ok(event)
        } else {
            decode_message(raw)
        }
    }

    fn sync_session(&self, session_id: &str) -> Result<(), MemoryError> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        sync_session_locked(&transaction, &self.events_dir, session_id)?;
        transaction.commit()?;
        Ok(())
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
        validate_fact(key.as_ref(), value.as_ref())?;
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

fn sync_session_locked(
    transaction: &Transaction<'_>,
    events_dir: &Path,
    session_id: &str,
) -> Result<(), MemoryError> {
    let path = event_path(events_dir, session_id)?;
    let (legacy_count, indexed_end): (i64, i64) = transaction.query_row(
        "SELECT COUNT(*) FILTER (WHERE event_offset IS NULL),
                    COALESCE(MAX(event_offset + event_length), 0)
             FROM messages WHERE session_id=?1",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let file_size = match fs::metadata(&path) {
        Ok(meta) => i64::try_from(meta.len())
            .map_err(|_| MemoryError::InvalidValue("event log too large".into()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    if legacy_count == 0 && file_size == indexed_end {
        return Ok(());
    }
    if file_size < indexed_end {
        return Err(MemoryError::InvalidValue(format!(
            "missing JSONL events for session {session_id}"
        )));
    }
    let mut present = HashMap::new();
    if file_size > 0 {
        let mut reader = BufReader::new(File::open(&path)?);
        let mut offset = 0_i64;
        loop {
            let mut line = Vec::new();
            let length = reader.read_until(b'\n', &mut line)?;
            if length == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                return Err(MemoryError::InvalidValue("incomplete JSONL event".into()));
            }
            let event: StoredMessage = serde_json::from_slice(&line)?;
            if event.session_id != session_id {
                return Err(MemoryError::InvalidValue("JSONL session mismatch".into()));
            }
            if present
                .insert(
                    event.id,
                    (offset, i64::try_from(length).unwrap_or(i64::MAX)),
                )
                .is_some()
            {
                return Err(MemoryError::InvalidValue("duplicate JSONL event id".into()));
            }
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND session_id=?2)",
                params![event.id, session_id],
                |row| row.get(0),
            )?;
            if exists {
                transaction.execute("UPDATE messages SET content='',metadata='null',event_offset=?2,event_length=?3 WHERE id=?1",
                        params![event.id, offset, length])?;
            } else {
                transaction.execute("INSERT INTO messages(id,session_id,role,kind,content,metadata,created_at,event_offset,event_length) VALUES (?1,?2,?3,?4,'','null',?5,?6,?7)",
                        params![event.id, session_id, event.role.as_str(), event.kind.as_str(), event.created_at, offset, length])?;
            }
            if event.kind == MessageKind::AgentState {
                transaction.execute("INSERT OR IGNORE INTO agent_states(message_id,session_id,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
                        params![event.id, session_id, event.content, serde_json::to_string(&event.metadata)?, event.created_at])?;
            }
            offset += i64::try_from(length).unwrap_or(i64::MAX);
        }
    }
    let legacy = {
        let mut statement = transaction.prepare("SELECT id,session_id,role,kind,content,metadata,created_at,event_offset,event_length FROM messages WHERE session_id=?1 AND event_offset IS NULL ORDER BY id")?;
        let rows = statement.query_map([session_id], map_raw_message)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for raw in legacy {
        if present.contains_key(&raw.0) {
            continue;
        }
        let event = decode_message(raw)?;
        let (offset, length) = append_event(events_dir, &event)?;
        transaction.execute("UPDATE messages SET content='',metadata='null',event_offset=?2,event_length=?3 WHERE id=?1", params![event.id, offset, length])?;
        if event.kind == MessageKind::AgentState {
            transaction.execute("INSERT OR IGNORE INTO agent_states(message_id,session_id,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
                    params![event.id, session_id, event.content, serde_json::to_string(&event.metadata)?, event.created_at])?;
        }
    }
    Ok(())
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

type RawMessage = (
    i64,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
);

fn map_raw_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMessage> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn event_path(events_dir: &Path, session_id: &str) -> Result<PathBuf, MemoryError> {
    let id = Uuid::parse_str(session_id)
        .map_err(|_| MemoryError::InvalidValue("invalid session id".into()))?;
    Ok(events_dir.join(format!("{id}.jsonl")))
}

fn append_event(events_dir: &Path, event: &StoredMessage) -> Result<(i64, i64), MemoryError> {
    fs::create_dir_all(events_dir)?;
    let path = event_path(events_dir, &event.session_id)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let offset = i64::try_from(file.metadata()?.len())
        .map_err(|_| MemoryError::InvalidValue("event log too large".into()))?;
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_data()?;
    let length = i64::try_from(line.len())
        .map_err(|_| MemoryError::InvalidValue("event too large".into()))?;
    Ok((offset, length))
}

impl Drop for MemoryStore {
    fn drop(&mut self) {
        if self.ephemeral_events {
            let _ = fs::remove_dir_all(&self.events_dir);
        }
    }
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
        let log = fs::read_to_string(root.join("sessions").join(format!("{}.jsonl", session.id)))
            .unwrap();
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
}
