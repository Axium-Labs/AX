//! Explicit storage scopes and conservative user-authored memory extraction.
use crate::{MemoryError, MemoryStore};
use lexical::LexicalFeatures;
use rusqlite::params;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

/// Minimum normalized similarity for a non-pinned fact to be retrieved.
///
/// This is a **noise floor, not a confidence gate**: it drops facts whose only
/// overlap is an incidental n-gram (an unrelated query measures `0.00`–`0.03`
/// against a fact in the same language), and keeps every fact that shares a
/// real word or term (measured `0.05`–`1.00` across English, Chinese, Japanese
/// and mixed queries). Ranking then orders what survives, and the injected
/// block tells the model to use a fact only when it is actually relevant — the
/// semantic decision stays with the model rather than with a lexical score.
pub const MIN_RELEVANCE: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    Global,
    Project,
    Session,
}
impl MemoryScope {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Session => "session",
        }
    }
}
/// Semantic lifetime, independent of storage scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    Preference,
    #[default]
    Fact,
    Decision,
    Task,
    Reference,
    Experience,
}
impl MemoryType {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Fact => "fact",
            Self::Decision => "decision",
            Self::Task => "task",
            Self::Reference => "reference",
            Self::Experience => "experience",
        }
    }
    /// Preferences remain stable until explicitly replaced.
    #[must_use]
    pub fn freshness(self, updated_at: i64, now: i64) -> f64 {
        let days = match self {
            Self::Preference => return 1.0,
            Self::Fact | Self::Decision | Self::Reference => 180.0,
            Self::Experience => 30.0,
            Self::Task => 3.0,
        };
        0.5_f64.powf(seconds_between(now, updated_at).max(0.0) / (days * 86_400.0))
    }
}
fn default_confidence() -> u8 {
    50
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryRecord {
    #[serde(default)]
    pub memory_type: MemoryType,
    #[serde(default)]
    pub usage_count: u32,
    /// Explicit confidence on a 0..=100 scale.
    #[serde(default = "default_confidence")]
    pub confidence: u8,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    #[serde(default)]
    pub superseded: bool,
    #[serde(default)]
    pub expired: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    pub key: String,
    pub value: String,
    pub scope: MemoryScope,
    pub owner: String,
    pub source: String,
    pub updated_at: i64,
    pub always_include: bool,
}
impl Default for MemoryRecord {
    fn default() -> Self {
        Self {
            key: String::new(),
            value: String::new(),
            scope: MemoryScope::Session,
            owner: String::new(),
            source: String::new(),
            updated_at: 0,
            always_include: false,
            memory_type: MemoryType::Fact,
            usage_count: 0,
            confidence: 50,
            last_used_at: None,
            superseded: false,
            expired: false,
            tags: vec![],
            paths: vec![],
        }
    }
}
/// Summary and precomputed lexical features; details are read separately by key.
#[derive(Clone, Debug)]
pub struct MemoryIndexEntry {
    pub memory: MemoryRecord,
    features: LexicalFeatures,
}

impl MemoryStore {
    /// Update or delete a fact only if its value still matches what the caller read.
    ///
    /// # Errors
    /// Returns an error for invalid data, stale updates, or database failures.
    pub fn change_fact(
        &self,
        record: &MemoryRecord,
        expected: Option<&str>,
        delete: bool,
    ) -> Result<(), MemoryError> {
        use rusqlite::OptionalExtension;
        let tx = self.connection.unchecked_transaction()?;
        let current: Option<String> = tx
            .query_row(
                "SELECT value FROM scoped_memories WHERE scope=?1 AND owner=?2 AND key=?3",
                params![record.scope.key(), record.owner, record.key],
                |row| row.get(0),
            )
            .optional()?;
        if current.as_deref() != expected || (delete && current.is_none()) {
            return Err(MemoryError::InvalidValue(
                "Memory changed or is missing; list it again before editing".into(),
            ));
        }
        if delete {
            self.forget_scoped(record.scope, &record.owner, &record.key)?;
        } else {
            self.remember_scoped(record)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Rebind legacy path-owned facts to a portable project ID.
    /// Only a project-local database may adopt a single unmatched legacy owner.
    ///
    /// # Errors
    /// Returns a database error if the migration cannot commit.
    pub fn migrate_project_owner(
        &self,
        id: &str,
        old_path: &str,
        project_local: bool,
    ) -> Result<(), MemoryError> {
        let tx = self.connection.unchecked_transaction()?;
        let owners = {
            let mut query =
                tx.prepare("SELECT DISTINCT owner FROM scoped_memories WHERE scope='project'")?;
            query
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let legacy = if owners.iter().any(|owner| owner == old_path) {
            Some(old_path)
        } else if project_local && owners.len() == 1 && uuid::Uuid::parse_str(&owners[0]).is_err() {
            Some(owners[0].as_str())
        } else {
            None
        };
        if let Some(old) = legacy {
            let migrated: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM project_memory_migrations WHERE owner=?1)",
                [old],
                |row| row.get(0),
            )?;
            if !migrated {
                for record in self.scoped_memories(MemoryScope::Project, old)? {
                    if validate_fact(&record.key, &record.value).is_err() {
                        continue;
                    }
                    tx.execute("INSERT OR IGNORE INTO scoped_memories(scope,owner,key,value,source,updated_at,always_include,memory_type,usage_count,confidence,last_used_at,superseded,expired,tags,paths)
                        VALUES ('project',?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)", params![id, record.key, record.value, record.source, record.updated_at,record.always_include,record.memory_type.key(),record.usage_count,record.confidence,record.last_used_at,record.superseded,record.expired,serde_json::to_string(&record.tags)?,serde_json::to_string(&record.paths)?])?;
                }
                tx.execute(
                    "INSERT INTO project_memory_migrations(owner,project_id) VALUES (?1,?2)",
                    params![old, id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Store one fact in an explicitly owned scope.
    ///
    /// # Errors
    /// Returns a database error or an invalid scope owner error.
    pub fn remember_scoped(&self, memory: &MemoryRecord) -> Result<(), MemoryError> {
        validate_fact(&memory.key, &memory.value)?;
        validate_labels(memory)?;
        if memory.confidence > 100 {
            return Err(MemoryError::InvalidValue(
                "confidence must be 0..=100".into(),
            ));
        }
        if memory.always_include && memory.scope != MemoryScope::Global {
            return Err(MemoryError::InvalidValue(
                "Only global preferences can be explicitly included in every task".into(),
            ));
        }
        if (memory.scope == MemoryScope::Global) != memory.owner.is_empty() || memory.key.is_empty()
        {
            return Err(MemoryError::InvalidValue(
                "invalid scope owner or empty key".into(),
            ));
        }
        if memory.scope == MemoryScope::Session {
            let exists: bool = self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [&memory.owner],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(MemoryError::InvalidValue("unknown memory session".into()));
            }
        }
        self.connection.execute("INSERT INTO scoped_memories(scope,owner,key,value,source,always_include,memory_type,usage_count,confidence,last_used_at,superseded,expired,tags,paths)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
            ON CONFLICT(scope,owner,key) DO UPDATE SET value=excluded.value,source=excluded.source,always_include=excluded.always_include,memory_type=excluded.memory_type,confidence=excluded.confidence,superseded=excluded.superseded,expired=excluded.expired,tags=excluded.tags,paths=excluded.paths,updated_at=unixepoch()",
            params![memory.scope.key(), memory.owner, memory.key, memory.value, memory.source,
                memory.always_include, memory.memory_type.key(), memory.usage_count, memory.confidence,
                memory.last_used_at, memory.superseded, memory.expired,
                serde_json::to_string(&memory.tags)?, serde_json::to_string(&memory.paths)?])?;
        warm_fact_features(&memory.key, &memory.value);
        self.update_memory_index(memory)?;
        Ok(())
    }
    /// Read facts from exactly one scope and owner.
    ///
    /// # Errors
    /// Returns a database error or an invalid scope owner error.
    pub fn scoped_memories(
        &self,
        scope: MemoryScope,
        owner: &str,
    ) -> Result<Vec<MemoryRecord>, MemoryError> {
        let _access = self
            .access
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut query = self.connection.prepare(&format!("SELECT {RECORD_COLUMNS} FROM scoped_memories WHERE scope=?1 AND owner=?2 ORDER BY updated_at DESC,key"))?;
        query
            .query_map(params![scope.key(), owner], |row| {
                read_record(row, scope, owner)
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    /// Load active summaries for exactly one scope owner.
    /// # Errors
    /// Returns database errors.
    pub fn memory_index(
        &self,
        scope: MemoryScope,
        owner: &str,
    ) -> Result<Vec<MemoryIndexEntry>, MemoryError> {
        // Imported/legacy entries derive features once, lazily within the selected owner.
        let pending = {
            let mut query = self.connection.prepare(&format!("SELECT {RECORD_COLUMNS} FROM scoped_memories WHERE scope=?1 AND owner=?2 AND superseded=0 AND expired=0 AND key IN (SELECT key FROM memory_index WHERE scope=?1 AND owner=?2 AND features IS NULL)"))?;
            query
                .query_map(params![scope.key(), owner], |row| {
                    read_record(row, scope, owner)
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for record in pending {
            self.update_memory_index(&record)?;
        }
        let mut query = self.connection.prepare("SELECT m.key,i.summary,m.source,m.updated_at,m.always_include,m.memory_type,m.usage_count,m.confidence,m.last_used_at,m.superseded,m.expired,m.tags,m.paths,i.features FROM scoped_memories m JOIN memory_index i USING(scope,owner,key) WHERE m.scope=?1 AND m.owner=?2 AND m.superseded=0 AND m.expired=0 AND i.valid=1 ORDER BY m.key")?;
        let rows = query
            .query_map(params![scope.key(), owner], |row| {
                Ok((read_record(row, scope, owner)?, row.get::<_, String>(13)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(memory, features)| {
                Ok(MemoryIndexEntry {
                    memory,
                    features: cached_index_features(&features)?,
                })
            })
            .collect()
    }

    fn update_memory_index(&self, record: &MemoryRecord) -> Result<(), MemoryError> {
        let valid =
            validate_fact(&record.key, &record.value).is_ok() && validate_labels(record).is_ok();
        let features = memory_features(record);
        self.connection.execute(
            "UPDATE memory_index SET features=?4,valid=?5 WHERE scope=?1 AND owner=?2 AND key=?3",
            params![
                record.scope.key(),
                record.owner,
                record.key,
                serde_json::to_string(&features)?,
                valid
            ],
        )?;
        Ok(())
    }
    /// Fetch one detail record without reading the rest of a scope.
    /// # Errors
    /// Returns database errors.
    pub fn read_memory(
        &self,
        scope: MemoryScope,
        owner: &str,
        key: &str,
    ) -> Result<Option<MemoryRecord>, MemoryError> {
        use rusqlite::OptionalExtension;
        self.connection.query_row(&format!("SELECT {RECORD_COLUMNS} FROM scoped_memories WHERE scope=?1 AND owner=?2 AND key=?3"),
            params![scope.key(), owner, key], |row| read_record(row, scope, owner)).optional().map_err(Into::into)
    }
    /// Record actual use without changing the content's freshness timestamp.
    /// # Errors
    /// Returns database errors.
    pub fn mark_memory_used(
        &self,
        scope: MemoryScope,
        owner: &str,
        key: &str,
    ) -> Result<(), MemoryError> {
        self.connection.execute("UPDATE scoped_memories SET usage_count=min(usage_count+1,4294967295),last_used_at=unixepoch() WHERE scope=?1 AND owner=?2 AND key=?3 AND superseded=0 AND expired=0", params![scope.key(), owner, key])?;
        Ok(())
    }
    /// Remove every remembered fact in this store: all scoped memories and the
    /// legacy long-term table they were migrated from. Raw session history,
    /// summaries and Evolution evidence are untouched. Legacy categories are
    /// marked migrated so the old table cannot resurrect a deleted fact.
    ///
    /// # Errors
    /// Returns a database error; the deletion is all-or-nothing.
    pub fn clear_all_memories(&self) -> Result<usize, MemoryError> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let scoped = tx.execute("DELETE FROM scoped_memories", [])?;
        let legacy = tx.execute("DELETE FROM long_term_memory", [])?;
        for category in ["project", "global"] {
            tx.execute(
                "INSERT OR IGNORE INTO memory_migrations(category) VALUES (?1)",
                [category],
            )?;
        }
        tx.commit()?;
        Ok(scoped + legacy)
    }
    /// Remove one fact without affecting other scopes.
    ///
    /// # Errors
    /// Returns a database error or an invalid scope owner error.
    pub fn forget_scoped(
        &self,
        scope: MemoryScope,
        owner: &str,
        key: &str,
    ) -> Result<bool, MemoryError> {
        Ok(self.connection.execute(
            "DELETE FROM scoped_memories WHERE scope=?1 AND owner=?2 AND key=?3",
            params![scope.key(), owner, key],
        )? > 0)
    }
}

/// Additive, transactional upgrade from any previous scoped-memory schema.
pub(crate) fn migrate_memory_index(connection: &rusqlite::Connection) -> Result<(), MemoryError> {
    let ready: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('scoped_memories') WHERE name='paths') AND EXISTS(SELECT 1 FROM pragma_table_info('memory_index') WHERE name='features')", [], |row| row.get(0))?;
    if ready {
        return Ok(());
    }
    let tx =
        rusqlite::Transaction::new_unchecked(connection, rusqlite::TransactionBehavior::Immediate)?;
    let had_type: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('scoped_memories') WHERE name='memory_type')", [], |row| row.get(0))?;
    for (column, definition) in [
        (
            "memory_type",
            "TEXT NOT NULL DEFAULT 'fact' CHECK(memory_type IN ('preference','fact','decision','task','reference','experience'))",
        ),
        (
            "usage_count",
            "INTEGER NOT NULL DEFAULT 0 CHECK(usage_count BETWEEN 0 AND 4294967295)",
        ),
        (
            "confidence",
            "INTEGER NOT NULL DEFAULT 50 CHECK(confidence BETWEEN 0 AND 100)",
        ),
        ("last_used_at", "INTEGER"),
        ("superseded", "INTEGER NOT NULL DEFAULT 0"),
        ("expired", "INTEGER NOT NULL DEFAULT 0"),
        ("tags", "TEXT NOT NULL DEFAULT '[]'"),
        ("paths", "TEXT NOT NULL DEFAULT '[]'"),
    ] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('scoped_memories') WHERE name=?1)",
            [column],
            |row| row.get(0),
        )?;
        if !exists {
            tx.execute(
                &format!("ALTER TABLE scoped_memories ADD COLUMN {column} {definition}"),
                [],
            )?;
        }
    }
    if !had_type {
        tx.execute("UPDATE scoped_memories SET memory_type='preference' WHERE always_include=1 OR key LIKE 'preference.%'", [])?;
    }
    // Triggers keep all older SQL write paths (including backup imports) coherent.
    tx.execute_batch("CREATE TABLE IF NOT EXISTS memory_index (
        scope TEXT NOT NULL,owner TEXT NOT NULL,key TEXT NOT NULL,summary TEXT NOT NULL,features TEXT,valid INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY(scope,owner,key));
        CREATE TRIGGER IF NOT EXISTS memory_index_insert AFTER INSERT ON scoped_memories BEGIN
            INSERT OR REPLACE INTO memory_index VALUES(new.scope,new.owner,new.key,substr(new.value,1,160),NULL,1);
        END;
        CREATE TRIGGER IF NOT EXISTS memory_index_update AFTER UPDATE OF value,key,owner,scope,tags,paths ON scoped_memories BEGIN
            DELETE FROM memory_index WHERE scope=old.scope AND owner=old.owner AND key=old.key;
            INSERT OR REPLACE INTO memory_index VALUES(new.scope,new.owner,new.key,substr(new.value,1,160),NULL,1);
        END;
        CREATE TRIGGER IF NOT EXISTS memory_index_delete AFTER DELETE ON scoped_memories BEGIN
            DELETE FROM memory_index WHERE scope=old.scope AND owner=old.owner AND key=old.key;
        END;
        INSERT OR IGNORE INTO memory_index SELECT scope,owner,key,substr(value,1,160),NULL,1 FROM scoped_memories;")?;
    tx.commit()?;
    Ok(())
}

fn validate_labels(record: &MemoryRecord) -> Result<(), MemoryError> {
    if record.tags.len() + record.paths.len() > 32 {
        return Err(MemoryError::InvalidValue(
            "too many memory tags/paths".into(),
        ));
    }
    for label in record.tags.iter().chain(&record.paths) {
        validate_fact("label", label)?;
        if label.chars().count() > 200 {
            return Err(MemoryError::InvalidValue(
                "memory label exceeds 200 characters".into(),
            ));
        }
    }
    Ok(())
}

const RECORD_COLUMNS: &str = "key,value,source,updated_at,always_include,memory_type,usage_count,confidence,last_used_at,superseded,expired,tags,paths";
fn read_record(
    row: &rusqlite::Row<'_>,
    scope: MemoryScope,
    owner: &str,
) -> rusqlite::Result<MemoryRecord> {
    let kind: String = row.get(5)?;
    Ok(MemoryRecord {
        key: row.get(0)?,
        value: row.get(1)?,
        source: row.get(2)?,
        updated_at: row.get(3)?,
        always_include: row.get(4)?,
        scope,
        owner: owner.into(),
        memory_type: serde_json::from_value(serde_json::Value::String(kind)).unwrap_or_default(),
        usage_count: row.get(6)?,
        confidence: row.get(7)?,
        last_used_at: row.get(8)?,
        superseded: row.get(9)?,
        expired: row.get(10)?,
        tags: serde_json::from_str(&row.get::<_, String>(11)?).unwrap_or_default(),
        paths: serde_json::from_str(&row.get::<_, String>(12)?).unwrap_or_default(),
    })
}

/// Validate every durable fact at the storage boundary.
///
/// # Errors
/// Returns an error for oversized, empty, or credential-like data.
pub fn validate_fact(key: &str, value: &str) -> Result<(), MemoryError> {
    let field = key.to_lowercase().replace(['-', '.', ' '], "_");
    let content = value.to_lowercase();
    let sensitive_key = [
        "password",
        "passwd",
        "api_key",
        "apikey",
        "secret",
        "access_token",
        "refresh_token",
        "authorization",
        "credential",
        "密码",
        "密钥",
    ]
    .iter()
    .any(|part| field.contains(part))
        || field == "token"
        || field.ends_with("_token");
    let sensitive_value = [
        "-----begin",
        "bearer ",
        "ghp_",
        "github_pat_",
        "password=",
        "password:",
        "api_key=",
        "api_key:",
        "api key=",
        "secret=",
        "secret:",
        "token=",
        "密码",
        "密钥",
    ]
    .iter()
    .any(|part| content.contains(part))
        || content
            .split(|c: char| c.is_whitespace() || matches!(c, '=' | ':' | '"' | '\''))
            .any(|part| part.starts_with("sk-") && part.len() >= 20);
    if key.trim().is_empty()
        || value.trim().is_empty()
        || key.chars().count() > 100
        || value.chars().count() > 1000
        || sensitive_key
        || sensitive_value
    {
        return Err(MemoryError::InvalidValue(
            "Invalid memory fact: empty, oversized, or credential-like data".into(),
        ));
    }
    Ok(())
}

/// Parse explicit structured declarations only. Unscoped writes are session-local.
/// Natural language intent is handled by the model through the memory tool.
#[must_use]
pub fn extract_user_memories(input: &str) -> Vec<(MemoryScope, String, String)> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (scope, rest) = [
                ("global:", MemoryScope::Global),
                ("project:", MemoryScope::Project),
                ("session:", MemoryScope::Session),
                ("全局：", MemoryScope::Global),
                ("项目：", MemoryScope::Project),
                ("本次：", MemoryScope::Session),
            ]
            .into_iter()
            .find_map(|(prefix, scope)| line.strip_prefix(prefix).map(|rest| (scope, rest.trim())))
            .unwrap_or((MemoryScope::Session, line));
            let declaration = rest
                .strip_prefix("remember ")
                .or_else(|| rest.strip_prefix("Remember "))
                .or_else(|| rest.strip_prefix("记住"))?;
            let (key, value) = declaration
                .trim()
                .trim_start_matches([':', '：'])
                .split_once('=')?;
            let (key, value) = (key.trim(), value.trim());
            validate_fact(key, value).ok()?;
            Some((scope, key.into(), value.into()))
        })
        .take(8)
        .collect()
}

/// Compatibility entry point for callers that already filtered scope ownership.
#[must_use]
pub fn retrieve(records: Vec<MemoryRecord>, query: &str) -> Vec<MemoryRecord> {
    retrieve_index(
        records
            .into_iter()
            .filter(|memory| validate_fact(&memory.key, &memory.value).is_ok())
            .map(|memory| MemoryIndexEntry {
                features: memory_features(&memory),
                memory,
            })
            .collect(),
        query,
        unix_now(),
    )
}
#[must_use]
pub fn unix_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}
/// Recall at most 32 lexical/exact/tag/path matches, then lightly rerank.
/// Filter ownership with `memory_index` before calling this function.
#[must_use]
pub fn retrieve_index(entries: Vec<MemoryIndexEntry>, query: &str, now: i64) -> Vec<MemoryRecord> {
    let features = LexicalFeatures::from_text(query);
    let normalized = lexical::normalize(query);
    let mut recalled = entries
        .into_iter()
        .filter(|entry| !entry.memory.superseded && !entry.memory.expired)
        .filter(|entry| validate_labels(&entry.memory).is_ok())
        .filter(|entry| validate_fact(&entry.memory.key, &entry.memory.value).is_ok())
        .map(|entry| {
            let record = entry.memory;
            let pinned = record.always_include && record.scope == MemoryScope::Global;
            let similarity = entry.features.similarity(&features);
            let exact = std::iter::once(&record.key)
                .chain(&record.tags)
                .chain(&record.paths)
                .any(|term| {
                    let term = lexical::normalize(term);
                    !features.is_empty() && !term.is_empty() && normalized.contains(&term)
                });
            let relevance = if exact {
                similarity.max(0.8)
            } else {
                similarity
            };
            (pinned, relevance, record)
        })
        .filter(|(pinned, relevance, _)| *pinned || *relevance >= MIN_RELEVANCE)
        .collect::<Vec<_>>();
    recalled.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| a.2.key.cmp(&b.2.key))
    });
    recalled.truncate(32);
    let mut ranked = recalled
        .into_iter()
        .map(|(pinned, relevance, record)| {
            let usage = f64::from(record.usage_count).ln_1p() / 32_f64.ln_1p();
            let score = relevance
                + 0.05 * usage.min(1.0)
                + 0.05 * f64::from(record.confidence) / 100.0
                + 0.10 * record.memory_type.freshness(record.updated_at, now);
            (pinned, score, record)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.total_cmp(&a.1))
            .then_with(|| b.2.updated_at.cmp(&a.2.updated_at))
            .then_with(|| a.2.key.cmp(&b.2.key))
    });
    ranked.into_iter().map(|(_, _, record)| record).collect()
}
#[allow(clippy::cast_precision_loss)]
fn seconds_between(newest: i64, updated_at: i64) -> f64 {
    newest.saturating_sub(updated_at) as f64
}

/// Features of stored facts, keyed by the exact text they were derived from.
///
/// Facts are written rarely and compared on every turn, so their features are
/// derived once — when the fact is written, or on first use for facts that were
/// already stored — and reused. Keying on the text means an edited fact simply
/// misses and is recomputed. The cache is bounded: on overflow it is cleared
/// rather than grown.
static FACT_FEATURES: OnceLock<Mutex<HashMap<String, LexicalFeatures>>> = OnceLock::new();
/// Upper bound on cached fact features before the cache is cleared.
const MAX_CACHED_FACTS: usize = 1_024;

fn fact_features(key: &str, value: &str) -> LexicalFeatures {
    let text = fact_text(key, value);
    let cache = FACT_FEATURES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut entries) = cache.lock() else {
        return LexicalFeatures::from_text(&text);
    };
    if let Some(features) = entries.get(&text) {
        return features.clone();
    }
    let features = LexicalFeatures::from_text(&text);
    if entries.len() >= MAX_CACHED_FACTS {
        entries.clear();
    }
    entries.insert(text, features.clone());
    features
}

fn memory_features(record: &MemoryRecord) -> LexicalFeatures {
    let routing_text = format!(
        "{} {} {}",
        record.value,
        record.tags.join(" "),
        record.paths.join(" ")
    );
    fact_features(&record.key, &routing_text)
}

fn cached_index_features(serialized: &str) -> Result<LexicalFeatures, MemoryError> {
    let key = format!("index:{serialized}");
    let cache = FACT_FEATURES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut entries) = cache.lock() else {
        return Ok(serde_json::from_str(serialized)?);
    };
    if let Some(features) = entries.get(&key) {
        return Ok(features.clone());
    }
    let features: LexicalFeatures = serde_json::from_str(serialized)?;
    if entries.len() >= MAX_CACHED_FACTS {
        entries.clear();
    }
    entries.insert(key, features.clone());
    Ok(features)
}

/// Derives a fact's features ahead of the next query, so a write pays the cost
/// once instead of every following turn.
fn warm_fact_features(key: &str, value: &str) {
    fact_features(key, value);
}

fn fact_text(key: &str, value: &str) -> String {
    format!("{key} {value}")
}

#[cfg(test)]
fn cached_fact_count() -> usize {
    FACT_FEATURES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_or(0, |entries| entries.len())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fact(scope: MemoryScope, owner: &str, key: &str, value: &str) -> MemoryRecord {
        MemoryRecord {
            scope,
            owner: owner.into(),
            key: key.into(),
            value: value.into(),
            source: "test".into(),
            updated_at: 0,
            always_include: false,
            ..Default::default()
        }
    }

    #[test]
    fn type_aware_decay_uses_elapsed_wall_clock() {
        let now = 180 * 86_400;
        assert!((MemoryType::Preference.freshness(0, now) - 1.0).abs() < f64::EPSILON);
        for kind in [
            MemoryType::Fact,
            MemoryType::Decision,
            MemoryType::Reference,
        ] {
            assert!((kind.freshness(0, now) - 0.5).abs() < 1e-12);
        }
        assert!((MemoryType::Experience.freshness(0, 30 * 86_400) - 0.5).abs() < 1e-12);
        assert!((MemoryType::Task.freshness(0, 3 * 86_400) - 0.5).abs() < 1e-12);
        assert!(MemoryType::Task.freshness(0, now) < MemoryType::Experience.freshness(0, now));
        assert!((MemoryType::Task.freshness(now + 100, now) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn inactive_memories_are_excluded_even_when_pinned() {
        let mut superseded = fact(MemoryScope::Global, "", "old", "cargo test");
        superseded.always_include = true;
        superseded.superseded = true;
        let mut expired = superseded.clone();
        expired.superseded = false;
        expired.expired = true;
        let active = fact(MemoryScope::Global, "", "active", "cargo test");
        let ranked = retrieve(vec![superseded, expired, active], "cargo test");
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].key, "active");
    }

    #[test]
    fn index_is_owned_and_details_are_lazy() {
        let store = MemoryStore::open_in_memory().unwrap();
        let session = store.create_session("current").unwrap();
        let other = store.create_session("other").unwrap();
        let body = format!("{} cargo test 中文", "long detail ".repeat(30));
        for (scope, owner, key) in [
            (MemoryScope::Global, "", "global"),
            (MemoryScope::Project, "current", "project"),
            (MemoryScope::Project, "other", "hidden-project"),
            (MemoryScope::Session, session.id.as_str(), "session"),
            (MemoryScope::Session, other.id.as_str(), "hidden-session"),
        ] {
            store
                .remember_scoped(&fact(scope, owner, key, &body))
                .unwrap();
        }
        let mut entries = store.memory_index(MemoryScope::Global, "").unwrap();
        entries.extend(store.memory_index(MemoryScope::Project, "current").unwrap());
        entries.extend(
            store
                .memory_index(MemoryScope::Session, &session.id)
                .unwrap(),
        );
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .all(|entry| entry.memory.value.chars().count() == 160)
        );
        let ranked = retrieve_index(entries, "cargo test 中文", unix_now());
        assert_eq!(ranked.len(), 3);
        assert!(ranked.iter().all(|row| !row.key.starts_with("hidden")));
        assert_eq!(
            store
                .read_memory(MemoryScope::Project, "current", "project")
                .unwrap()
                .unwrap()
                .value,
            body
        );
        assert!(
            store
                .read_memory(MemoryScope::Project, "current", "hidden-project")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn tag_path_matches_and_usage_rerank_do_not_expand_recall() {
        let mut records = (0..40)
            .map(|i| {
                fact(
                    MemoryScope::Project,
                    "p",
                    &format!("item-{i:02}"),
                    "cargo test",
                )
            })
            .collect::<Vec<_>>();
        records[39].usage_count = u32::MAX;
        records[39].confidence = 100;
        records[1].usage_count = 32;
        let ranked = retrieve(records, "cargo test");
        assert_eq!(ranked.len(), 32);
        assert_eq!(ranked[0].key, "item-01");
        assert!(ranked.iter().all(|row| row.key != "item-39"));
        let mut tagged = fact(MemoryScope::Project, "p", "guide", "unrelated content");
        tagged.tags = vec!["部署".into()];
        tagged.paths = vec!["src/parser.rs".into()];
        assert_eq!(retrieve(vec![tagged.clone()], "部署 workflow").len(), 1);
        assert_eq!(retrieve(vec![tagged], "check src/parser.rs").len(), 1);
    }

    #[test]
    fn persisted_features_recall_terms_beyond_summary_and_partial_paths() {
        let store = MemoryStore::open_in_memory().unwrap();
        store
            .remember_scoped(&MemoryRecord {
                scope: MemoryScope::Project,
                owner: "p".into(),
                key: "guide".into(),
                value: format!("{}unicode tokenizer", "detail ".repeat(35)),
                tags: vec!["中文部署".into()],
                paths: vec!["src/parser.rs".into()],
                ..Default::default()
            })
            .unwrap();
        for query in ["unicode tokenizer", "parser.rs", "中文部署 workflow"] {
            let ranked = retrieve_index(
                store.memory_index(MemoryScope::Project, "p").unwrap(),
                query,
                unix_now(),
            );
            assert_eq!(ranked.first().map(|row| row.key.as_str()), Some("guide"));
        }
        let pending: i64 = store
            .connection
            .query_row(
                "SELECT count(*) FROM memory_index WHERE features IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn confidence_and_type_freshness_order_equal_matches() {
        let now = unix_now();
        let mut old_task = scoped_fact(MemoryScope::Project, "p", "a", now - 30 * 86_400);
        old_task.memory_type = MemoryType::Task;
        let mut preference = old_task.clone();
        preference.key = "z".into();
        preference.memory_type = MemoryType::Preference;
        assert_eq!(
            retrieve(vec![old_task, preference], "cargo test")[0].key,
            "z"
        );
        let mut low = scoped_fact(MemoryScope::Project, "p", "a", now);
        low.confidence = 0;
        let mut high = low.clone();
        high.key = "z".into();
        high.confidence = 100;
        assert_eq!(retrieve(vec![low, high], "cargo test")[0].key, "z");
    }

    #[test]
    fn index_tracks_edits_deletes_and_usage_without_rejuvenation() {
        let store = MemoryStore::open_in_memory().unwrap();
        let mut record = fact(MemoryScope::Project, "p", "build", "cargo test");
        record.memory_type = MemoryType::Decision;
        record.confidence = 91;
        record.tags = vec!["测试".into()];
        record.paths = vec!["src/lib.rs".into()];
        store.remember_scoped(&record).unwrap();
        let saved = store
            .read_memory(MemoryScope::Project, "p", "build")
            .unwrap()
            .unwrap();
        store
            .mark_memory_used(MemoryScope::Project, "p", "build")
            .unwrap();
        let used = store
            .read_memory(MemoryScope::Project, "p", "build")
            .unwrap()
            .unwrap();
        assert_eq!(used.usage_count, 1);
        assert!(used.last_used_at.is_some());
        assert_eq!(saved.updated_at, used.updated_at);
        record.value = "cargo check".into();
        record.superseded = true;
        store
            .change_fact(&record, Some("cargo test"), false)
            .unwrap();
        assert!(
            store
                .memory_index(MemoryScope::Project, "p")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .read_memory(MemoryScope::Project, "p", "build")
                .unwrap()
                .unwrap()
                .usage_count,
            1
        );
        store
            .forget_scoped(MemoryScope::Project, "p", "build")
            .unwrap();
        let count: i64 = store
            .connection
            .query_row("SELECT count(*) FROM memory_index", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn old_database_upgrade_preserves_values_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!("ax-old-memory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memory.sqlite3");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE scoped_memories(scope TEXT NOT NULL,owner TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,source TEXT NOT NULL,updated_at INTEGER NOT NULL,PRIMARY KEY(scope,owner,key));
            INSERT INTO scoped_memories VALUES('project','p','build','cargo test 中文','user/old',123);
            INSERT INTO scoped_memories VALUES('global','','preference.language','中文','legacy',456);
            PRAGMA user_version=4;").unwrap();
        drop(connection);
        for _ in 0..2 {
            let store = MemoryStore::open(&path).unwrap();
            let record = store
                .read_memory(MemoryScope::Project, "p", "build")
                .unwrap()
                .unwrap();
            assert_eq!(record.value, "cargo test 中文");
            assert_eq!(record.source, "user/old");
            assert_eq!(record.updated_at, 123);
            assert_eq!(record.memory_type, MemoryType::Fact);
            assert_eq!(record.confidence, 50);
            assert_eq!(record.usage_count, 0);
            assert_eq!(
                store.memory_index(MemoryScope::Project, "p").unwrap().len(),
                1
            );
            assert_eq!(
                store
                    .read_memory(MemoryScope::Global, "", "preference.language")
                    .unwrap()
                    .unwrap()
                    .memory_type,
                MemoryType::Preference
            );
            let version: i64 = store
                .connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 7);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_invalid_detail_is_not_exposed_through_short_summary() {
        let store = MemoryStore::open_in_memory().unwrap();
        let value = format!("{}Bearer private", "ordinary context ".repeat(15));
        store.connection.execute("INSERT INTO scoped_memories(scope,owner,key,value,source) VALUES ('global','','unsafe',?1,'legacy')",[value]).unwrap();
        assert!(
            store
                .memory_index(MemoryScope::Global, "")
                .unwrap()
                .is_empty()
        );
        let valid: i64 = store
            .connection
            .query_row(
                "SELECT valid FROM memory_index WHERE key='unsafe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(valid, 0);
    }

    #[test]
    fn old_serialized_records_have_safe_metadata_defaults() {
        let record:MemoryRecord=serde_json::from_value(serde_json::json!({"key":"build","value":"cargo test","scope":"project","owner":"p","source":"legacy","updated_at":123,"always_include":false})).unwrap();
        assert_eq!(record.memory_type, MemoryType::Fact);
        assert_eq!(record.confidence, 50);
        assert!(!record.superseded && !record.expired);
        assert!(record.last_used_at.is_none());
    }

    #[test]
    fn every_write_path_rejects_credentials() {
        let store = MemoryStore::open_in_memory().unwrap();
        for (key, value) in [
            ("api_key", "abc123"),
            ("PASSWORD", "abc123"),
            ("note", "Bearer abc123"),
            ("note", "-----BEGIN PRIVATE KEY-----"),
        ] {
            let record = fact(MemoryScope::Global, "", key, value);
            assert!(store.remember_scoped(&record).is_err());
            assert!(store.change_fact(&record, None, false).is_err());
            assert!(store.remember(key, value, "global").is_err());
        }
        assert!(
            store
                .scoped_memories(MemoryScope::Global, "")
                .unwrap()
                .is_empty()
        );
        assert!(validate_fact("context.token_budget", "12000").is_ok());
        assert!(validate_fact("workflow", "Use task-based execution").is_ok());
    }

    #[test]
    fn updates_detect_conflicts_and_deletion_is_scoped() {
        let store = MemoryStore::open_in_memory().unwrap();
        let mut record = fact(MemoryScope::Project, "p", "response.detail", "brief");
        store.change_fact(&record, None, false).unwrap();
        store
            .remember_scoped(&fact(MemoryScope::Project, "other", &record.key, "brief"))
            .unwrap();
        record.value = "detailed".into();
        assert!(store.change_fact(&record, None, false).is_err());
        store.change_fact(&record, Some("brief"), false).unwrap();
        let saved = store.scoped_memories(MemoryScope::Project, "p").unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].value, "detailed");
        assert!(saved[0].updated_at > 0);
        assert!(store.change_fact(&record, Some("brief"), true).is_err());
        store.change_fact(&record, Some("detailed"), true).unwrap();
        assert!(
            store
                .scoped_memories(MemoryScope::Project, "p")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .scoped_memories(MemoryScope::Project, "other")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn only_explicitly_pinned_global_facts_ignore_relevance() {
        let mut preference = fact(MemoryScope::Global, "", "preference.language", "English");
        assert!(retrieve(vec![preference.clone()], "compile parser").is_empty());
        preference.always_include = true;
        assert_eq!(
            retrieve(vec![preference.clone()], "compile parser").len(),
            1
        );
        preference.scope = MemoryScope::Project;
        preference.owner = "p".into();
        assert!(
            MemoryStore::open_in_memory()
                .unwrap()
                .remember_scoped(&preference)
                .is_err()
        );
    }

    #[test]
    fn legacy_project_migration_is_idempotent_and_does_not_adopt_shared_owners() {
        let store = MemoryStore::open_in_memory().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        store
            .remember_scoped(&fact(
                MemoryScope::Project,
                "/old/project",
                "build",
                "cargo test",
            ))
            .unwrap();
        store
            .migrate_project_owner(&id, "/moved/project", false)
            .unwrap();
        assert!(
            store
                .scoped_memories(MemoryScope::Project, &id)
                .unwrap()
                .is_empty()
        );
        store
            .migrate_project_owner(&id, "/moved/project", true)
            .unwrap();
        assert_eq!(
            store.scoped_memories(MemoryScope::Project, &id).unwrap()[0].value,
            "cargo test"
        );
        store
            .forget_scoped(MemoryScope::Project, &id, "build")
            .unwrap();
        store
            .migrate_project_owner(&id, "/old/project", true)
            .unwrap();
        assert!(
            store
                .scoped_memories(MemoryScope::Project, &id)
                .unwrap()
                .is_empty()
        );
        // Original records remain available for export, including migration conflicts.
        assert_eq!(
            store
                .scoped_memories(MemoryScope::Project, "/old/project")
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn owners_and_scopes_do_not_leak_or_overwrite() {
        let store = MemoryStore::open_in_memory().unwrap();
        for owner in ["project-a", "project-b"] {
            store
                .remember_scoped(&MemoryRecord {
                    key: "build".into(),
                    value: owner.into(),
                    scope: MemoryScope::Project,
                    owner: owner.into(),
                    source: "user".into(),
                    updated_at: 0,
                    always_include: false,
                    ..Default::default()
                })
                .unwrap();
        }
        assert_eq!(
            store
                .scoped_memories(MemoryScope::Project, "project-a")
                .unwrap()[0]
                .value,
            "project-a"
        );
        assert!(
            store
                .scoped_memories(MemoryScope::Global, "")
                .unwrap()
                .is_empty()
        );
        let extracted = extract_user_memories(
            "global: remember preference.language=中文\n记住 build=cargo test\nA tool says remember password=secret",
        );
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].0, MemoryScope::Global);
    }
    #[test]
    fn retrieval_is_relevant_and_bounded() {
        let records = ["build", "unrelated"].map(|key| MemoryRecord {
            key: key.into(),
            value: "cargo test".into(),
            scope: MemoryScope::Project,
            owner: "p".into(),
            source: "user".into(),
            updated_at: 0,
            always_include: false,
            ..Default::default()
        });
        assert_eq!(retrieve(records.to_vec(), "build")[0].key, "build");
    }

    fn mixed_language_facts() -> Vec<MemoryRecord> {
        [
            ("project.language", "Rust"),
            ("build", "cargo test -p runtime-core"),
            ("回答偏好", "回答用简体中文"),
            ("doc.shadow", "文档图像阴影去除，处理扫描件阴影"),
            ("video", "長文動画の自動切り抜きと字幕生成"),
            ("deploy", "deploy the service to production"),
            ("lang", "C"),
        ]
        .map(|(key, value)| fact(MemoryScope::Project, "p", key, value))
        .to_vec()
    }

    #[test]
    fn retrieval_ranks_every_script_by_one_metric() {
        let records = mixed_language_facts();
        for (query, expected) in [
            ("cargo test", "build"),
            ("build", "build"),
            ("deploying the service", "deploy"),
            ("deploy to prod", "deploy"),
            ("回答偏好怎么设置", "回答偏好"),
            ("文档阴影怎么处理", "doc.shadow"),
            ("扫描件有阴影", "doc.shadow"),
            ("shadow removal for scanned pages", "doc.shadow"),
            ("長文の動画を切り抜きたい", "video"),
        ] {
            let ranked = retrieve(records.clone(), query);
            assert_eq!(
                ranked.first().map(|record| record.key.as_str()),
                Some(expected),
                "query {query:?} ranked {:?}",
                ranked.iter().map(|r| &r.key).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn retrieval_is_not_biased_by_query_length() {
        let records = mixed_language_facts();
        for query in ["文档阴影", "这个扫描件有阴影，帮我处理一下"] {
            assert_eq!(
                retrieve(records.clone(), query)[0].key,
                "doc.shadow",
                "query {query:?}"
            );
        }
    }

    #[test]
    fn retrieval_ignores_unrelated_and_signal_free_queries() {
        let records = mixed_language_facts();
        for query in [
            "项目用什么语言写的",
            "帮我部署到生产环境",
            "帮我写一个排序算法",
            "...",
            "   ",
        ] {
            assert!(
                retrieve(records.clone(), query).is_empty(),
                "query {query:?} retrieved {:?}",
                retrieve(records.clone(), query)
                    .iter()
                    .map(|r| &r.key)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn single_character_facts_are_reachable() {
        let records = mixed_language_facts();
        assert_eq!(retrieve(records, "c")[0].key, "lang");
    }

    fn scoped_fact(scope: MemoryScope, owner: &str, key: &str, updated_at: i64) -> MemoryRecord {
        MemoryRecord {
            updated_at,
            ..fact(scope, owner, key, "cargo test")
        }
    }

    #[test]
    fn scope_never_changes_rank_of_equally_relevant_facts() {
        let scopes = [
            MemoryScope::Global,
            MemoryScope::Project,
            MemoryScope::Session,
        ];
        for scope in scopes {
            let records = vec![
                scoped_fact(scope, "owner", "first", 0),
                scoped_fact(MemoryScope::Global, "", "second", 0),
            ];
            assert_eq!(retrieve(records, "cargo test")[0].key, "first");
        }
    }

    #[test]
    fn recency_orders_equally_relevant_facts() {
        let records = vec![
            scoped_fact(MemoryScope::Project, "p", "build", unix_now() - 86_400),
            scoped_fact(MemoryScope::Project, "p", "build", unix_now()),
        ];
        let ranked = retrieve(records, "cargo test");
        assert!(ranked[0].updated_at > ranked[1].updated_at);
    }

    #[test]
    fn pinned_global_facts_rank_first_and_ignore_relevance() {
        let mut pinned = fact(MemoryScope::Global, "", "preference.answer", "English");
        pinned.always_include = true;
        let records = vec![
            pinned,
            fact(MemoryScope::Session, "s", "build", "cargo test"),
        ];
        let ranked = retrieve(records, "cargo test");
        assert_eq!(ranked[0].key, "preference.answer");
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn relevance_outweighs_scope_and_recency_for_weak_matches() {
        // The strongest scope cannot lift a fact whose only overlap is
        // incidental above a fact that actually answers the query.
        let records = vec![
            fact(MemoryScope::Session, "s", "project.language", "Rust"),
            fact(
                MemoryScope::Global,
                "",
                "build",
                "cargo test -p runtime-core",
            ),
        ];
        assert_eq!(retrieve(records, "cargo test")[0].key, "build");
    }

    #[test]
    fn fact_features_are_cached_on_write_and_bounded() {
        let store = MemoryStore::open_in_memory().unwrap();
        let before = cached_fact_count();
        store
            .remember_scoped(&fact(
                MemoryScope::Project,
                "p",
                "cache.probe",
                "text unique to the feature cache test",
            ))
            .unwrap();
        assert!(
            cached_fact_count() > before,
            "writing a fact must precompute its features"
        );
        for index in 0..(MAX_CACHED_FACTS + 16) {
            fact_features("cache", &format!("probe value number {index}"));
        }
        // Other tests share this cache and run in parallel, so allow a small
        // overshoot instead of asserting an exact bound.
        assert!(cached_fact_count() <= MAX_CACHED_FACTS + 32);
    }
}
