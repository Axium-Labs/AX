//! Explicit storage scopes and conservative user-authored memory extraction.
use crate::{MemoryError, MemoryStore};
use rusqlite::params;

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

/// Rank relevant facts by lexical matches, breaking ties by update time.
#[must_use]
pub fn retrieve(mut records: Vec<MemoryRecord>, query: &str) -> Vec<MemoryRecord> {
    let terms = terms(query);
    records.sort_by_key(|record| std::cmp::Reverse((score(record, &terms), record.updated_at)));
    records
        .into_iter()
        .filter(|record| {
            score(record, &terms) > 0 && validate_fact(&record.key, &record.value).is_ok()
        })
        .take(32)
        .collect()
}
fn score(record: &MemoryRecord, terms: &[String]) -> usize {
    let haystack = format!("{} {}", record.key, record.value).to_lowercase();
    usize::from(record.scope == MemoryScope::Global && record.always_include)
        + terms
            .iter()
            .filter(|term| haystack.contains(term.as_str()))
            .count()
}
fn terms(query: &str) -> Vec<String> {
    let lower = query.to_lowercase();
    let mut terms = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.len() > 1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let chars = lower.chars().collect::<Vec<_>>();
    terms.extend(
        chars
            .windows(2)
            .filter(|pair| pair.iter().all(|c| !c.is_ascii() && !c.is_whitespace()))
            .map(|pair| pair.iter().collect()),
    );
    terms.sort();
    terms.dedup();
    terms
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
}
