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
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryRecord {
    pub key: String,
    pub value: String,
    pub scope: MemoryScope,
    pub owner: String,
    pub source: String,
    pub updated_at: i64,
    pub always_include: bool,
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
                    tx.execute("INSERT OR IGNORE INTO scoped_memories(scope,owner,key,value,source,updated_at)
                        VALUES ('project',?1,?2,?3,?4,?5)", params![id, record.key, record.value, record.source, record.updated_at])?;
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
        self.connection.execute("INSERT INTO scoped_memories(scope, owner, key, value, source, always_include) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(scope, owner, key) DO UPDATE SET value=excluded.value, source=excluded.source, always_include=excluded.always_include, updated_at=unixepoch()",
            params![memory.scope.key(), memory.owner, memory.key, memory.value, memory.source, memory.always_include])?;
        warm_fact_features(&memory.key, &memory.value);
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
        let mut query = self.connection.prepare("SELECT key,value,source,updated_at,always_include FROM scoped_memories WHERE scope=?1 AND owner=?2 ORDER BY updated_at DESC,key")?;
        let records = query.query_map(params![scope.key(), owner], |row| {
            Ok(MemoryRecord {
                key: row.get(0)?,
                value: row.get(1)?,
                source: row.get(2)?,
                updated_at: row.get(3)?,
                always_include: row.get(4)?,
                scope,
                owner: owner.to_owned(),
            })
        })?;
        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
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

/// Weights of the final memory ranking. Lexical relevance dominates so a fact
/// that actually answers the request outranks a merely recent or local one;
/// the other three terms order facts whose relevance is close or equal.
const RELEVANCE_WEIGHT: f64 = 0.60;
/// Weight of the scope term (Session > Project > Global, matching the
/// same-key precedence documented in `docs/memory.md`).
const SCOPE_WEIGHT: f64 = 0.15;
/// Weight of the recency term.
const RECENCY_WEIGHT: f64 = 0.15;
/// Weight of the importance term.
const IMPORTANCE_WEIGHT: f64 = 0.10;
/// Age at which the recency term halves (14 days), in seconds.
const RECENCY_HALF_LIFE_SECS: f64 = 14.0 * 24.0 * 60.0 * 60.0;

/// Rank facts for injection. Candidates come from lexical relevance —
/// [`MIN_RELEVANCE`] only rejects incidental overlap and never decides
/// meaning — and the final order combines four signals:
///
/// ```text
/// score = 0.60 · relevance        # shared lexical similarity, 0.0..=1.0
///       + 0.15 · scope priority   # Session 1.0, Project 0.7, Global 0.4
///       + 0.15 · recency          # halves every 14 days behind the newest candidate
///       + 0.10 · importance       # 1.0 pinned, 0.6 user-stated, 0.4 migrated/other
/// ```
///
/// Explicitly pinned global preferences bypass the relevance floor and rank
/// first; they still have to pass [`validate_fact`].
///
/// Ranking is language-independent: the query and every fact are compared with
/// the shared [`LexicalFeatures`] similarity, so a Chinese or Japanese request
/// is ranked by the same code path as an English one and no language produces
/// a systematically lower score.
#[must_use]
pub fn retrieve(records: Vec<MemoryRecord>, query: &str) -> Vec<MemoryRecord> {
    let features = LexicalFeatures::from_text(query);
    let newest = records.iter().map(|record| record.updated_at).max();
    let mut ranked = records
        .into_iter()
        .filter(|record| validate_fact(&record.key, &record.value).is_ok())
        .map(|record| {
            let pinned = record.always_include && record.scope == MemoryScope::Global;
            let relevance = fact_features(&record.key, &record.value).similarity(&features);
            (pinned, relevance, record)
        })
        .filter(|(pinned, relevance, _)| *pinned || *relevance >= MIN_RELEVANCE)
        .map(|(pinned, relevance, record)| {
            let importance = if pinned {
                1.0
            } else {
                provenance_importance(&record.source)
            };
            let score = RELEVANCE_WEIGHT * relevance
                + SCOPE_WEIGHT * scope_priority(record.scope)
                + RECENCY_WEIGHT * recency(record.updated_at, newest)
                + IMPORTANCE_WEIGHT * importance;
            (pinned, score, record)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.total_cmp(&left.1))
            .then_with(|| right.2.updated_at.cmp(&left.2.updated_at))
            .then_with(|| left.2.key.cmp(&right.2.key))
    });
    ranked
        .into_iter()
        .take(32)
        .map(|(_, _, record)| record)
        .collect()
}

/// More specific scopes win, matching the same-key precedence.
const fn scope_priority(scope: MemoryScope) -> f64 {
    match scope {
        MemoryScope::Session => 1.0,
        MemoryScope::Project => 0.7,
        MemoryScope::Global => 0.4,
    }
}

/// Freshness relative to the newest candidate, halving every 14 days. Relative
/// rather than wall-clock keeps ranking deterministic for a given input set.
fn recency(updated_at: i64, newest: Option<i64>) -> f64 {
    let Some(newest) = newest else {
        return 0.0;
    };
    0.5_f64.powf(seconds_between(newest, updated_at) / RECENCY_HALF_LIFE_SECS)
}

/// A second count stays exactly representable in `f64` up to 2^53 (about 285
/// million years), so the cast cannot lose meaningful precision.
#[allow(clippy::cast_precision_loss)]
fn seconds_between(newest: i64, updated_at: i64) -> f64 {
    newest.saturating_sub(updated_at) as f64
}

/// Facts the user stated rank above facts carried over from older storage.
fn provenance_importance(source: &str) -> f64 {
    if source.starts_with("user/") {
        0.6
    } else {
        0.4
    }
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
        }
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
    fn scope_priority_orders_equally_relevant_facts() {
        let records = vec![
            scoped_fact(MemoryScope::Global, "", "build", 0),
            scoped_fact(MemoryScope::Project, "p", "build", 0),
            scoped_fact(MemoryScope::Session, "s", "build", 0),
        ];
        let ranked = retrieve(records, "cargo test");
        assert_eq!(
            ranked.iter().map(|record| record.scope).collect::<Vec<_>>(),
            vec![
                MemoryScope::Session,
                MemoryScope::Project,
                MemoryScope::Global
            ]
        );
    }

    #[test]
    fn recency_orders_equally_relevant_facts() {
        let records = vec![
            scoped_fact(MemoryScope::Project, "p", "build", 0),
            scoped_fact(MemoryScope::Project, "p", "build", 86_400),
        ];
        let ranked = retrieve(records, "cargo test");
        assert_eq!(ranked[0].updated_at, 86_400);
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
