//! The storage failure taxonomy.

use thiserror::Error;

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
