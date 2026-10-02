//! Opening the store: the connection, the event directory and migrations.

use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::Connection;
use uuid::Uuid;

use crate::{MemoryError, migrations};

pub struct MemoryStore {
    pub(crate) connection: Connection,
    pub(crate) events_dir: PathBuf,
    pub(crate) ephemeral_events: bool,
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

    pub(crate) fn initialize(
        connection: Connection,
        events_dir: PathBuf,
        ephemeral_events: bool,
    ) -> Result<Self, MemoryError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        migrations::apply(&connection)?;
        Ok(Self {
            connection,
            events_dir,
            ephemeral_events,
        })
    }
}

impl Drop for MemoryStore {
    fn drop(&mut self) {
        if self.ephemeral_events {
            let _ = fs::remove_dir_all(&self.events_dir);
        }
    }
}
