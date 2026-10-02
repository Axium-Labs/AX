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
    pub(crate) access: std::sync::Arc<std::sync::Mutex<()>>,
}

impl MemoryStore {
    /// Opens or creates a memory database and applies lightweight migrations.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot open or migrate the database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MemoryError> {
        let path = path.as_ref();
        let access = access_for(path);
        let initialization_access = access.clone();
        let _initialization = initialization_access
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let connection = Connection::open(path)?;
        let events_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("sessions");
        Self::initialize(connection, events_dir, false, access)
    }

    /// Creates an isolated in-memory store, primarily for tests and ephemeral runs.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` initialization fails.
    pub fn open_in_memory() -> Result<Self, MemoryError> {
        let events_dir = std::env::temp_dir().join(format!("ax-session-events-{}", Uuid::new_v4()));
        let access = std::sync::Arc::new(std::sync::Mutex::new(()));
        Self::initialize(Connection::open_in_memory()?, events_dir, true, access)
    }

    pub(crate) fn initialize(
        connection: Connection,
        events_dir: PathBuf,
        ephemeral_events: bool,
        access: std::sync::Arc<std::sync::Mutex<()>>,
    ) -> Result<Self, MemoryError> {
        connection.busy_timeout(std::time::Duration::from_secs(30))?;
        migrations::apply(&connection)?;
        Ok(Self {
            connection,
            events_dir,
            ephemeral_events,
            access,
        })
    }
}

fn access_for(path: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    static REGISTRY: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Arc<std::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let registry = REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .entry(key)
        .or_insert_with(|| std::sync::Arc::new(std::sync::Mutex::new(())))
        .clone()
}

impl Drop for MemoryStore {
    fn drop(&mut self) {
        if self.ephemeral_events {
            let _ = fs::remove_dir_all(&self.events_dir);
        }
    }
}
