//! The `SQLite` schema, and the version each batch records.
//!
//! Definitions only: `crate::migrations` decides when each batch runs.
//! Every statement is additive, so an older database converges by reopening.

/// Base tables: sessions, messages, long-term memory and session summaries.
/// Records `user_version = 2`.
pub(crate) const BASE: &str = "PRAGMA foreign_keys = ON;
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
             PRAGMA user_version = 2;";

/// Scoped memories and legacy-migration bookkeeping. Records `user_version = 4`.
pub(crate) const SCOPED_MEMORIES: &str = "CREATE TABLE IF NOT EXISTS scoped_memories (
            scope TEXT NOT NULL CHECK(scope IN ('global','project','session')), owner TEXT NOT NULL,
            key TEXT NOT NULL, value TEXT NOT NULL, source TEXT NOT NULL,
            updated_at INTEGER NOT NULL DEFAULT (unixepoch()), PRIMARY KEY(scope, owner, key));
            CREATE TABLE IF NOT EXISTS memory_migrations (category TEXT PRIMARY KEY, migrated_at INTEGER NOT NULL DEFAULT (unixepoch()));
            PRAGMA user_version = 4;";

/// Project ownership of legacy memory. Records `user_version = 5`.
pub(crate) const PROJECT_MEMORY_MIGRATIONS: &str = "CREATE TABLE IF NOT EXISTS project_memory_migrations(owner TEXT PRIMARY KEY, project_id TEXT NOT NULL); PRAGMA user_version=5;";

/// Durable agent states, kept separate from the bounded message page.
/// Records `user_version = 7`.
pub(crate) const AGENT_STATES: &str = "CREATE TABLE IF NOT EXISTS agent_states (
            message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            content TEXT NOT NULL, metadata TEXT NOT NULL, created_at INTEGER NOT NULL
        ); CREATE INDEX IF NOT EXISTS idx_agent_states_session ON agent_states(session_id, message_id);
        PRAGMA user_version=7;";
