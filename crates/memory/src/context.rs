//! The compressed session summary and the effective-context watermark.
//!
//! Compression never deletes raw history: this module only records what was
//! fed to the model and how far it covered the raw transcript.

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    MemoryError, MemoryStore, StoredMessage, events::sync_session_locked, message::map_raw_message,
};

impl MemoryStore {
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
}
impl MemoryStore {
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
}
