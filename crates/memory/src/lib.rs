//! `SQLite` metadata and memory with per-session JSONL event history.
//!
//! Opening the store and running its small migrations are explicit operations;
//! merely linking this crate performs no filesystem or database work.
//!
//! Layout:
//!
//! - `store` — the `MemoryStore` façade: connection, event directory, open.
//! - `schema` / `migrations` — the additive `SQLite` schema and its order.
//! - `session` / `message` / `context` / `long_term` — the four
//!   stored concerns, one module each.
//! - `events` — the append-only JSONL stream and its index.
//! - `types` / `error` — pure data and the failure taxonomy.
//! - `scoped` / [`backup`] — scoped memories and portable archives.

mod context;
mod error;
mod events;
mod long_term;
mod message;
mod migrations;
mod schema;
mod session;
mod store;
mod types;

pub mod backup;
mod scoped;

pub use error::MemoryError;
pub use scoped::{
    MIN_RELEVANCE, MemoryIndexEntry, MemoryRecord, MemoryScope, MemoryType, extract_user_memories,
    retrieve, retrieve_index, unix_now, validate_fact,
};
pub use store::MemoryStore;
pub use types::{LongTermMemory, MessageKind, MessageRole, NewMessage, Session, StoredMessage};

#[cfg(test)]
mod tests;
