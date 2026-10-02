//! Additive schema migrations, applied in one fixed order when the store opens.
//!
//! A migration is only ever an added table, index or column, so an interrupted
//! open converges on the next one. No migration rewrites or drops data.

use rusqlite::{Connection, OptionalExtension};

use crate::{MemoryError, MemoryStore, schema, scoped};

/// Applies every migration in the order they were introduced.
///
/// All of it runs in one transaction: a fresh database pays one commit, not one
/// per batch. This is what keeps child session creation off the critical path.
pub(crate) fn apply(connection: &Connection) -> Result<(), MemoryError> {
    let mut journal_mode: String =
        connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if journal_mode != "memory" && journal_mode != "wal" {
        journal_mode = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    }
    if journal_mode != "memory" && journal_mode != "wal" {
        return Err(MemoryError::Database(
            rusqlite::Error::ToSqlConversionFailure(
                format!("SQLite journal_mode is {journal_mode}, expected wal").into(),
            ),
        ));
    }
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let tx = connection.unchecked_transaction()?;
    tx.execute_batch(schema::BASE)?;
    ensure_column(
        &tx,
        "session_summaries",
        "through_message_id",
        "ALTER TABLE session_summaries ADD COLUMN through_message_id INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        &tx,
        "session_summaries",
        "effective_context",
        "ALTER TABLE session_summaries ADD COLUMN effective_context TEXT",
    )?;
    tx.execute_batch(schema::SCOPED_MEMORIES)?;
    ensure_column(
        &tx,
        "scoped_memories",
        "always_include",
        "ALTER TABLE scoped_memories ADD COLUMN always_include INTEGER NOT NULL DEFAULT 0",
    )?;
    tx.execute_batch(schema::PROJECT_MEMORY_MIGRATIONS)?;
    for column in ["event_offset", "event_length"] {
        ensure_column(
            &tx,
            "messages",
            column,
            &format!("ALTER TABLE messages ADD COLUMN {column} INTEGER"),
        )?;
    }
    migrate_agent_states(&tx)?;
    tx.commit()?;
    scoped::migrate_memory_index(connection)?;
    Ok(())
}

/// Adds a column only when it is missing, so the check and the `ALTER` stay in
/// one place instead of drifting apart across call sites.
fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    alter: &str,
) -> Result<(), MemoryError> {
    let present: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name=?1"),
        [column],
        |row| row.get(0),
    )?;
    if present == 0 {
        connection.execute(alter, [])?;
    }
    Ok(())
}

/// Durable agent states are stored separately from the bounded message page,
/// so a resume can always recover the latest state even when it predates the
/// history page.
fn migrate_agent_states(connection: &Connection) -> Result<(), MemoryError> {
    connection.execute_batch(schema::AGENT_STATES)?;
    Ok(())
}

impl MemoryStore {
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
}
