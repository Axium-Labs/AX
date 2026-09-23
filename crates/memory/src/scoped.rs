//! Explicit storage scopes and conservative user-authored memory extraction.
use crate::{MemoryError, MemoryStore};
use rusqlite::params;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryRecord {
    pub key: String,
    pub value: String,
    pub scope: MemoryScope,
    pub owner: String,
    pub source: String,
}
impl MemoryStore {
    /// Store one fact in an explicitly owned scope.
    ///
    /// # Errors
    /// Returns a database error or an invalid scope owner error.
    pub fn remember_scoped(&self, memory: &MemoryRecord) -> Result<(), MemoryError> {
        if (memory.scope == MemoryScope::Global) != memory.owner.is_empty() || memory.key.is_empty()
        {
            return Err(MemoryError::InvalidValue(
                "invalid scope owner or empty key".into(),
            ));
        }
        if memory.scope == MemoryScope::Session && self.session(&memory.owner)?.is_none() {
            return Err(MemoryError::InvalidValue("unknown memory session".into()));
        }
        self.connection.execute("INSERT INTO scoped_memories(scope, owner, key, value, source) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(scope, owner, key) DO UPDATE SET value=excluded.value, source=excluded.source, updated_at=unixepoch()",
            params![memory.scope.key(), memory.owner, memory.key, memory.value, memory.source])?;
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
        let mut query = self.connection.prepare("SELECT key,value,source FROM scoped_memories WHERE scope=?1 AND owner=?2 ORDER BY updated_at DESC,key")?;
        let records = query.query_map(params![scope.key(), owner], |row| {
            Ok(MemoryRecord {
                key: row.get(0)?,
                value: row.get(1)?,
                source: row.get(2)?,
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

/// Extract only explicit user memory declarations, never model/tool assertions.
/// `remember key=value`, `记住 key=value`; prefixes `global:` / `session:` select scope.
#[must_use]
pub fn extract_user_memories(input: &str) -> Vec<(MemoryScope, String, String)> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (scope, line) = if let Some(rest) = line
                .strip_prefix("global:")
                .or_else(|| line.strip_prefix("全局："))
            {
                (MemoryScope::Global, rest.trim())
            } else if let Some(rest) = line
                .strip_prefix("session:")
                .or_else(|| line.strip_prefix("本次："))
            {
                (MemoryScope::Session, rest.trim())
            } else {
                (MemoryScope::Project, line)
            };
            let (key, value) = if let Some(statement) = line
                .strip_prefix("remember ")
                .or_else(|| line.strip_prefix("Remember "))
                .or_else(|| line.strip_prefix("记住"))
            {
                let statement = statement.trim().trim_start_matches([':', '：']).trim();
                match statement.split_once('=') {
                    Some((key, value)) => (key.trim().to_owned(), value.trim().to_owned()),
                    None => (fact_key(statement), statement.to_owned()),
                }
            } else if [
                "我偏好",
                "我习惯",
                "请以后",
                "这个项目使用",
                "我们约定",
                "I prefer ",
                "For this project, ",
            ]
            .iter()
            .any(|prefix| line.starts_with(prefix))
            {
                (fact_key(line), line.to_owned())
            } else {
                return None;
            };
            let lower = value.to_lowercase();
            if [
                "password", "api_key", "api key", "secret", "token=", "密码", "密钥",
            ]
            .iter()
            .any(|word| lower.contains(word))
            {
                return None;
            }
            if key.is_empty()
                || value.is_empty()
                || key.chars().count() > 100
                || value.chars().count() > 1000
            {
                return None;
            }
            Some((scope, key, value))
        })
        .take(8)
        .collect()
}

fn fact_key(statement: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    statement.hash(&mut hash);
    let prefix = if statement.contains("偏好")
        || statement.contains("习惯")
        || statement.contains("以后")
        || statement.contains("prefer")
    {
        "preference"
    } else {
        "fact"
    };
    format!("{prefix}.{:x}", hash.finish())
}

/// Rank by lexical relevance, with named preferences always eligible.
#[must_use]
pub fn retrieve(mut records: Vec<MemoryRecord>, query: &str) -> Vec<MemoryRecord> {
    let terms = terms(query);
    records.sort_by_key(|record| std::cmp::Reverse(score(record, &terms)));
    records
        .into_iter()
        .filter(|record| score(record, &terms) > 0)
        .take(8)
        .collect()
}
fn score(record: &MemoryRecord, terms: &[String]) -> usize {
    let haystack = format!("{} {}", record.key, record.value).to_lowercase();
    usize::from(record.key.starts_with("preference."))
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
    terms
}
#[cfg(test)]
mod tests {
    use super::*;
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
        });
        assert_eq!(retrieve(records.to_vec(), "build")[0].key, "build");
    }
}
