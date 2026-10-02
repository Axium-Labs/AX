//! Session lifecycle: creation, lookup, paging, rename and cascade delete.

use std::fs;

use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::{MemoryError, MemoryStore, Session};

impl MemoryStore {
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
        // Do not commit an invisible session while its raw transcript remains on disk.
        // An already removed index may still have an orphaned transcript to clean up.
        match fs::remove_file(self.event_path(id)?) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        transaction.commit()?;
        Ok(deleted)
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
