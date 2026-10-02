//! Legacy key/value long-term memory.

use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::{LongTermMemory, MemoryError, MemoryStore, scoped::validate_fact};

impl MemoryStore {
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
