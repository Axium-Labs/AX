//! Message append, paged load and decode.
//!
//! `SQLite` holds only the index for indexed messages; the body lives in the
//! session's JSONL event stream.

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    MemoryError, MemoryStore, MessageKind, MessageRole, NewMessage, StoredMessage,
    events::{append_event, sync_session_locked},
};

impl MemoryStore {
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

    /// Appends a run of messages as one transaction, one JSONL sync and one
    /// commit, instead of one of each per message.
    ///
    /// Durability matches [`Self::append_message`]: the JSONL stream is flushed
    /// before the transaction commits, so a crash can only leave an orphaned
    /// event, which `sync_session_locked` re-indexes on the next open. A child
    /// checkpoint writes a batch per save, so this is what keeps its fsync
    /// count constant instead of linear in the message count.
    ///
    /// # Errors
    /// Returns an error when serialization or the transaction fails.
    pub fn append_batch(
        &mut self,
        session_id: &str,
        messages: &[NewMessage],
    ) -> Result<Vec<StoredMessage>, MemoryError> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        sync_session_locked(&transaction, &self.events_dir, session_id)?;
        let mut stored = Vec::with_capacity(messages.len());
        let mut jsonl = Vec::new();
        let mut offset = crate::events::events_size(&self.events_dir, session_id)?;
        for message in messages {
            let NewMessage {
                role,
                kind,
                content,
                metadata,
            } = message.clone();
            transaction.execute(
                "INSERT INTO messages (session_id, role, kind, content, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session_id, role.as_str(), kind.as_str(), "", "null"],
            )?;
            let id = transaction.last_insert_rowid();
            let created_at: i64 = transaction.query_row(
                "SELECT created_at FROM messages WHERE id=?1",
                [id],
                |row| row.get(0),
            )?;
            let stored_message = StoredMessage {
                id,
                session_id: session_id.to_owned(),
                role,
                kind,
                content,
                metadata,
                created_at,
            };
            let mut line = serde_json::to_vec(&stored_message)?;
            line.push(b'\n');
            let length = i64::try_from(line.len())
                .map_err(|_| MemoryError::InvalidValue("event too large".into()))?;
            jsonl.extend_from_slice(&line);
            transaction.execute(
                "UPDATE messages SET event_offset=?2,event_length=?3 WHERE id=?1",
                params![id, offset, length],
            )?;
            offset = offset.saturating_add(length);
            if kind == MessageKind::AgentState {
                transaction.execute("INSERT INTO agent_states(message_id,session_id,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
                    params![id, session_id, stored_message.content, serde_json::to_string(&stored_message.metadata)?, created_at])?;
            }
            stored.push(stored_message);
        }
        transaction.execute(
            "UPDATE sessions SET updated_at = unixepoch() WHERE id = ?1",
            [session_id],
        )?;
        // Flush the JSONL stream once, before the commit.
        crate::events::append_event_buffer(&self.events_dir, session_id, &jsonl)?;
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

    /// Load the latest durable state in a namespace, independent of history pages.
    ///
    /// # Errors
    /// Returns storage or decoding failures.
    pub fn latest_agent_state(
        &self,
        session_id: &str,
        prefix: &str,
    ) -> Result<Option<StoredMessage>, MemoryError> {
        self.sync_session(session_id)?;
        let mut statement = self.connection.prepare(
            "SELECT a.message_id,a.session_id,m.role,'agent_state',a.content,a.metadata,a.created_at,NULL,NULL
             FROM agent_states a JOIN messages m ON m.id=a.message_id
             WHERE a.session_id=?1 AND substr(a.content,1,length(?2))=?2
             ORDER BY a.message_id DESC LIMIT 1",
        )?;
        let raw = statement
            .query_row(params![session_id, prefix], map_raw_message)
            .optional()?;
        raw.map(decode_message).transpose()
    }
}
pub(crate) type RawMessage = (
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

pub(crate) fn map_raw_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMessage> {
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

pub(crate) fn decode_message(raw: RawMessage) -> Result<StoredMessage, MemoryError> {
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
