//! Versioned portable archives over the existing memory repository.
use crate::{
    MemoryError, MemoryRecord, MemoryScope, MemoryStore, MessageKind, MessageRole, StoredMessage,
    validate_fact,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;
use zip::{ZipArchive, ZipWriter, write::FileOptions};

const FORMAT: &str = "axpack";
const VERSION: u32 = 1;
const MAX_ENTRY: u64 = 128 * 1024 * 1024;
const MAX_TOTAL: u64 = 512 * 1024 * 1024;
const FILES: [&str; 6] = [
    "metadata.json",
    "memories.jsonl",
    "legacy_memories.jsonl",
    "sessions.jsonl",
    "messages.jsonl",
    "summaries.jsonl",
];

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("storage: {0}")]
    Storage(#[from] MemoryError),
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid axpack: {0}")]
    Invalid(String),
}

#[derive(Clone, Copy, Debug)]
pub struct ExportSelection {
    pub memory: bool,
    pub sessions: bool,
}
impl ExportSelection {
    #[must_use]
    pub const fn all() -> Self {
        Self {
            memory: true,
            sessions: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportReport {
    pub sessions: usize,
    pub messages: usize,
    pub summaries: usize,
    pub memories: usize,
    pub legacy_memories: usize,
    pub session_conflicts: Vec<String>,
    pub memory_conflicts: Vec<String>,
    pub orphan_session_memories: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileInfo {
    sha256: String,
    records: usize,
    bytes: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    ax_version: String,
    created_at: i64,
    includes: Vec<String>,
    files: BTreeMap<String, FileInfo>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    project_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackSession {
    id: String,
    title: String,
    created_at: i64,
    updated_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackMessage {
    session_id: String,
    ordinal: usize,
    role: MessageRole,
    kind: MessageKind,
    content: String,
    metadata: Value,
    created_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackSummary {
    session_id: String,
    content: String,
    compressed_message_count: i64,
    through_ordinal: usize,
    effective_context: Option<String>,
    updated_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackLegacyMemory {
    origin: String,
    id: String,
    key: String,
    value: String,
    category: String,
    created_at: i64,
    updated_at: i64,
}
struct Package {
    manifest: Manifest,
    metadata: Metadata,
    memories: Vec<MemoryRecord>,
    legacy: Vec<PackLegacyMemory>,
    sessions: Vec<PackSession>,
    messages: Vec<PackMessage>,
    summaries: Vec<PackSummary>,
}

pub struct ExportService<'a> {
    pub project: &'a MemoryStore,
    pub global: &'a MemoryStore,
    pub project_id: &'a str,
}

impl ExportService<'_> {
    /// Export selected portable records to a new archive.
    ///
    /// # Errors
    /// Returns an error for unreadable history or archive I/O failure.
    #[allow(clippy::too_many_lines)]
    pub fn export(
        &self,
        path: &Path,
        selection: ExportSelection,
    ) -> Result<ImportReport, BackupError> {
        if path.exists() {
            return Err(BackupError::Invalid(format!(
                "destination exists: {}",
                path.display()
            )));
        }
        let mut package = Package {
            manifest: Manifest {
                format: FORMAT.into(),
                version: VERSION,
                ax_version: env!("CARGO_PKG_VERSION").into(),
                created_at: now(),
                includes: Vec::new(),
                files: BTreeMap::new(),
            },
            metadata: Metadata {
                project_id: self.project_id.into(),
            },
            memories: Vec::new(),
            legacy: Vec::new(),
            sessions: Vec::new(),
            messages: Vec::new(),
            summaries: Vec::new(),
        };
        let sessions = all_sessions(self.project)?;
        if selection.sessions {
            package.manifest.includes.push("sessions".into());
            for session in &sessions {
                package.sessions.push(PackSession {
                    id: session.id.clone(),
                    title: session.title.clone(),
                    created_at: session.created_at,
                    updated_at: session.updated_at,
                });
                let messages = all_messages(self.project, &session.id)?;
                let ordinal = messages
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (m.id, i + 1))
                    .collect::<HashMap<_, _>>();
                package
                    .messages
                    .extend(messages.into_iter().enumerate().map(|(i, m)| PackMessage {
                        session_id: session.id.clone(),
                        ordinal: i + 1,
                        role: m.role,
                        kind: m.kind,
                        content: m.content,
                        metadata: m.metadata,
                        created_at: m.created_at,
                    }));
                let summary = self.project.connection.query_row(
                    "SELECT content,compressed_message_count,through_message_id,effective_context,updated_at FROM session_summaries WHERE session_id=?1",
                    [&session.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, i64>(4)?))
                ).optional()?;
                if let Some((content, count, through, effective_context, updated_at)) = summary {
                    let through_ordinal = if through == 0 {
                        0
                    } else {
                        *ordinal.get(&through).ok_or_else(|| {
                            BackupError::Invalid("summary watermark has no message".into())
                        })?
                    };
                    package.summaries.push(PackSummary {
                        session_id: session.id.clone(),
                        content,
                        compressed_message_count: count,
                        through_ordinal,
                        effective_context,
                        updated_at,
                    });
                }
            }
        }
        if selection.memory {
            package.manifest.includes.push("memory".into());
            package
                .memories
                .extend(self.global.scoped_memories(MemoryScope::Global, "")?);
            package.memories.extend(
                self.project
                    .scoped_memories(MemoryScope::Project, self.project_id)?,
            );
            for session in &sessions {
                package.memories.extend(
                    self.project
                        .scoped_memories(MemoryScope::Session, &session.id)?,
                );
            }
            package
                .legacy
                .extend(legacy_records(self.global, "global")?);
            package
                .legacy
                .extend(legacy_records(self.project, "project")?);
            package
                .memories
                .retain(|r| validate_fact(&r.key, &r.value).is_ok());
            package
                .legacy
                .retain(|r| validate_fact(&r.key, &r.value).is_ok());
        }
        let report = ImportReport {
            sessions: package.sessions.len(),
            messages: package.messages.len(),
            summaries: package.summaries.len(),
            memories: package.memories.len(),
            legacy_memories: package.legacy.len(),
            ..ImportReport::default()
        };
        write_package(path, &mut package)?;
        Ok(report)
    }
}

fn legacy_records(store: &MemoryStore, origin: &str) -> Result<Vec<PackLegacyMemory>, BackupError> {
    let mut statement = store.connection.prepare(
        "SELECT id,key,value,category,created_at,updated_at FROM long_term_memory ORDER BY key",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(PackLegacyMemory {
                origin: origin.into(),
                id: row.get(0)?,
                key: row.get(1)?,
                value: row.get(2)?,
                category: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn all_sessions(store: &MemoryStore) -> Result<Vec<crate::Session>, BackupError> {
    let mut result = Vec::new();
    loop {
        let offset = u32::try_from(result.len())
            .map_err(|_| BackupError::Invalid("too many sessions".into()))?;
        let page = store.list_sessions(256, offset)?;
        if page.is_empty() {
            break;
        }
        result.extend(page);
    }
    Ok(result)
}
fn all_messages(store: &MemoryStore, session_id: &str) -> Result<Vec<StoredMessage>, BackupError> {
    let mut pages = Vec::new();
    let mut before = None;
    loop {
        let page = store.load_messages(session_id, before, 256)?;
        if page.is_empty() {
            break;
        }
        before = Some(page[0].id);
        pages.push(page);
    }
    pages.reverse();
    Ok(pages.into_iter().flatten().collect())
}
fn lines<T: Serialize>(items: &[T]) -> Result<Vec<u8>, BackupError> {
    let mut bytes = Vec::new();
    for item in items {
        serde_json::to_writer(&mut bytes, item)?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}
fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| i64::try_from(time.as_secs()).unwrap_or(i64::MAX))
}

fn write_package(path: &Path, package: &mut Package) -> Result<(), BackupError> {
    let data = [
        (FILES[0], serde_json::to_vec(&package.metadata)?, 1),
        (FILES[1], lines(&package.memories)?, package.memories.len()),
        (FILES[2], lines(&package.legacy)?, package.legacy.len()),
        (FILES[3], lines(&package.sessions)?, package.sessions.len()),
        (FILES[4], lines(&package.messages)?, package.messages.len()),
        (
            FILES[5],
            lines(&package.summaries)?,
            package.summaries.len(),
        ),
    ];
    for (name, bytes, records) in &data {
        package.manifest.files.insert(
            (*name).into(),
            FileInfo {
                sha256: checksum(bytes),
                records: *records,
                bytes: bytes.len(),
            },
        );
    }
    let total = data.iter().try_fold(0_u64, |size, (name, bytes, _)| {
        let length = u64::try_from(bytes.len())
            .map_err(|_| BackupError::Invalid("entry too large".into()))?;
        if length > MAX_ENTRY {
            return Err(BackupError::Invalid(format!("oversized entry: {name}")));
        }
        size.checked_add(length)
            .ok_or_else(|| BackupError::Invalid("archive too large".into()))
    })?;
    if total > MAX_TOTAL {
        return Err(BackupError::Invalid(
            "archive exceeds 512 MB uncompressed".into(),
        ));
    }
    let temp = path.with_extension(format!("axpack-{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<(), BackupError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        let mut writer = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("manifest.json", options)?;
        writer.write_all(&serde_json::to_vec_pretty(&package.manifest)?)?;
        for (name, bytes, _) in data {
            writer.start_file(name, options)?;
            writer.write_all(&bytes)?;
        }
        writer.finish()?.sync_all()?;
        if path.exists() {
            return Err(BackupError::Invalid(
                "destination appeared during export".into(),
            ));
        }
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn read_entry(archive: &mut ZipArchive<File>, name: &str) -> Result<Vec<u8>, BackupError> {
    let entry = archive.by_name(name)?;
    if entry.size() > MAX_ENTRY {
        return Err(BackupError::Invalid(format!("oversized entry: {name}")));
    }
    let mut bytes = Vec::new();
    entry.take(MAX_ENTRY + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ENTRY {
        return Err(BackupError::Invalid(format!("oversized entry: {name}")));
    }
    Ok(bytes)
}
fn parse_lines<T: DeserializeOwned>(bytes: &[u8]) -> Result<Vec<T>, BackupError> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(BackupError::from))
        .collect()
}
fn read_package(path: &Path) -> Result<Package, BackupError> {
    let mut archive = ZipArchive::new(File::open(path)?)?;
    if archive.len() != FILES.len() + 1 {
        return Err(BackupError::Invalid("unexpected archive entries".into()));
    }
    let manifest: Manifest = serde_json::from_slice(&read_entry(&mut archive, "manifest.json")?)?;
    if manifest.format != FORMAT {
        return Err(BackupError::Invalid(format!(
            "unsupported format: {}",
            manifest.format
        )));
    }
    if manifest.version != VERSION {
        return Err(BackupError::Invalid(format!(
            "unsupported format version: {}",
            manifest.version
        )));
    }
    if manifest.ax_version.trim().is_empty() || manifest.created_at < 0 {
        return Err(BackupError::Invalid("invalid manifest provenance".into()));
    }
    if manifest.files.len() != FILES.len()
        || manifest
            .includes
            .iter()
            .any(|s| s != "memory" && s != "sessions")
        || manifest.includes.is_empty()
        || manifest.includes.len() > 2
        || manifest.includes.iter().collect::<HashSet<_>>().len() != manifest.includes.len()
    {
        return Err(BackupError::Invalid(
            "invalid file list or included data types".into(),
        ));
    }
    let mut contents = BTreeMap::new();
    let mut total = 0_u64;
    for name in FILES {
        let info = manifest
            .files
            .get(name)
            .ok_or_else(|| BackupError::Invalid(format!("missing manifest entry {name}")))?;
        let bytes = read_entry(&mut archive, name)?;
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_TOTAL || info.bytes != bytes.len() || info.sha256 != checksum(&bytes) {
            return Err(BackupError::Invalid(format!(
                "checksum or size mismatch: {name}"
            )));
        }
        contents.insert(name, bytes);
    }
    let metadata: Metadata = serde_json::from_slice(&contents[FILES[0]])?;
    if Uuid::parse_str(&metadata.project_id).is_err() {
        return Err(BackupError::Invalid("invalid project ID".into()));
    }
    let memories = parse_lines::<MemoryRecord>(&contents[FILES[1]])?;
    let legacy = parse_lines::<PackLegacyMemory>(&contents[FILES[2]])?;
    let sessions = parse_lines::<PackSession>(&contents[FILES[3]])?;
    let messages = parse_lines::<PackMessage>(&contents[FILES[4]])?;
    let summaries = parse_lines::<PackSummary>(&contents[FILES[5]])?;
    for (name, count) in [
        (FILES[1], memories.len()),
        (FILES[2], legacy.len()),
        (FILES[3], sessions.len()),
        (FILES[4], messages.len()),
        (FILES[5], summaries.len()),
    ] {
        if manifest.files[name].records != count {
            return Err(BackupError::Invalid(format!(
                "record count mismatch: {name}"
            )));
        }
    }
    if manifest.files[FILES[0]].records != 1 {
        return Err(BackupError::Invalid("metadata count mismatch".into()));
    }
    let package = Package {
        manifest,
        metadata,
        memories,
        legacy,
        sessions,
        messages,
        summaries,
    };
    validate(&package)?;
    Ok(package)
}
fn validate(package: &Package) -> Result<(), BackupError> {
    let has_memory = package.manifest.includes.iter().any(|s| s == "memory");
    let has_sessions = package.manifest.includes.iter().any(|s| s == "sessions");
    if (!has_memory && (!package.memories.is_empty() || !package.legacy.is_empty()))
        || (!has_sessions
            && (!package.sessions.is_empty()
                || !package.messages.is_empty()
                || !package.summaries.is_empty()))
    {
        return Err(BackupError::Invalid("records contradict includes".into()));
    }
    let mut ids = HashSet::new();
    for session in &package.sessions {
        if Uuid::parse_str(&session.id).is_err()
            || !ids.insert(&session.id)
            || session.title.trim().is_empty()
        {
            return Err(BackupError::Invalid("duplicate or invalid session".into()));
        }
    }
    let mut ordinals: HashMap<&str, usize> = HashMap::new();
    for message in &package.messages {
        if !ids.contains(&message.session_id) {
            return Err(BackupError::Invalid("message without session".into()));
        }
        let expected = ordinals.entry(&message.session_id).or_default();
        *expected += 1;
        if message.ordinal != *expected {
            return Err(BackupError::Invalid(
                "message ordinals must be contiguous".into(),
            ));
        }
    }
    let mut summary_ids = HashSet::new();
    for summary in &package.summaries {
        if !ids.contains(&summary.session_id)
            || !summary_ids.insert(&summary.session_id)
            || summary.through_ordinal > *ordinals.get(summary.session_id.as_str()).unwrap_or(&0)
        {
            return Err(BackupError::Invalid("invalid summary reference".into()));
        }
        if let Some(value) = &summary.effective_context {
            serde_json::from_str::<Value>(value)?;
        }
    }
    let mut keys = HashSet::new();
    for record in &package.memories {
        validate_fact(&record.key, &record.value)?;
        let owner_valid = match record.scope {
            MemoryScope::Global => record.owner.is_empty(),
            MemoryScope::Project => record.owner == package.metadata.project_id,
            MemoryScope::Session => Uuid::parse_str(&record.owner).is_ok(),
        };
        if !owner_valid || !keys.insert((record.scope.key(), &record.owner, &record.key)) {
            return Err(BackupError::Invalid(
                "invalid or duplicate scoped memory".into(),
            ));
        }
    }
    let mut legacy_keys = HashSet::new();
    for record in &package.legacy {
        validate_fact(&record.key, &record.value)?;
        if !matches!(record.origin.as_str(), "global" | "project")
            || Uuid::parse_str(&record.id).is_err()
            || !legacy_keys.insert((&record.origin, &record.key))
        {
            return Err(BackupError::Invalid(
                "invalid or duplicate legacy memory".into(),
            ));
        }
    }
    Ok(())
}

pub struct ImportService<'a> {
    pub project: &'a mut MemoryStore,
    pub global_path: &'a Path,
    pub target_project_id: &'a str,
}

impl ImportService<'_> {
    /// Validate and analyze an archive using read-only database handles.
    ///
    /// # Errors
    /// Returns an error for malformed archives or unreadable databases.
    pub fn dry_run(
        path: &Path,
        project_path: &Path,
        global_path: &Path,
        target_project_id: Option<&str>,
    ) -> Result<ImportReport, BackupError> {
        let package = read_package(path)?;
        let project = readonly(project_path)?;
        let global = readonly(global_path)?;
        plan(
            &package,
            project.as_ref(),
            global.as_ref(),
            target_project_id,
        )
    }

    /// Merge validated records without replacing current records.
    ///
    /// # Errors
    /// Returns an error and rolls back database changes on validation or write failure.
    pub fn import(&mut self, path: &Path) -> Result<ImportReport, BackupError> {
        let package = read_package(path)?;
        if Uuid::parse_str(self.target_project_id).is_err() {
            return Err(BackupError::Invalid("invalid target project ID".into()));
        }
        if let Some(parent) = self.global_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let global = MemoryStore::open(self.global_path)?;
        let report = plan(
            &package,
            Some(&self.project.connection),
            Some(&global.connection),
            Some(self.target_project_id),
        )?;
        drop(global);
        self.project.connection.execute(
            "ATTACH DATABASE ?1 AS ax_global",
            [self.global_path.to_string_lossy().as_ref()],
        )?;
        let result = merge(self.project, &package, &report, self.target_project_id);
        let detach = self
            .project
            .connection
            .execute_batch("DETACH DATABASE ax_global");
        result?;
        detach?;
        Ok(report)
    }
}

fn readonly(path: &Path) -> Result<Option<Connection>, BackupError> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?))
}
fn exists(
    connection: Option<&Connection>,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<bool, BackupError> {
    let Some(connection) = connection else {
        return Ok(false);
    };
    match connection.query_row(sql, args, |row| row.get::<_, bool>(0)) {
        Ok(value) => Ok(value),
        Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            if message.starts_with("no such table:") =>
        {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}
fn plan(
    package: &Package,
    project: Option<&Connection>,
    global: Option<&Connection>,
    target_project_id: Option<&str>,
) -> Result<ImportReport, BackupError> {
    let mut report = ImportReport::default();
    let mut new_sessions = HashSet::new();
    for session in &package.sessions {
        if exists(
            project,
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            &[&session.id],
        )? {
            report.session_conflicts.push(session.id.clone());
        } else {
            report.sessions += 1;
            new_sessions.insert(session.id.as_str());
        }
    }
    report.messages = package
        .messages
        .iter()
        .filter(|m| new_sessions.contains(m.session_id.as_str()))
        .count();
    report.summaries = package
        .summaries
        .iter()
        .filter(|s| new_sessions.contains(s.session_id.as_str()))
        .count();
    for record in &package.memories {
        let owner = match record.scope {
            MemoryScope::Global => "",
            MemoryScope::Project => target_project_id.unwrap_or(""),
            MemoryScope::Session => {
                if !new_sessions.contains(record.owner.as_str()) {
                    report.orphan_session_memories += 1;
                    continue;
                }
                &record.owner
            }
        };
        let connection = if record.scope == MemoryScope::Global {
            global
        } else {
            project
        };
        if exists(
            connection,
            "SELECT EXISTS(SELECT 1 FROM scoped_memories WHERE scope=?1 AND owner=?2 AND key=?3)",
            &[&record.scope.key(), &owner, &record.key],
        )? {
            report.memory_conflicts.push(format!(
                "{}:{}:{}",
                record.scope.key(),
                owner,
                record.key
            ));
        } else {
            report.memories += 1;
        }
    }
    for record in &package.legacy {
        let connection = if record.origin == "global" {
            global
        } else {
            project
        };
        if exists(
            connection,
            "SELECT EXISTS(SELECT 1 FROM long_term_memory WHERE key=?1 OR id=?2)",
            &[&record.key, &record.id],
        )? {
            report
                .memory_conflicts
                .push(format!("legacy:{}:{}", record.origin, record.key));
        } else {
            report.legacy_memories += 1;
        }
    }
    Ok(report)
}

struct FileCleanup {
    paths: Vec<PathBuf>,
    keep: bool,
}
impl FileCleanup {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            keep: false,
        }
    }
}
impl Drop for FileCleanup {
    fn drop(&mut self) {
        if !self.keep {
            for path in &self.paths {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[allow(clippy::too_many_lines)]
fn merge(
    store: &mut MemoryStore,
    package: &Package,
    report: &ImportReport,
    target_project_id: &str,
) -> Result<(), BackupError> {
    fs::create_dir_all(&store.events_dir)?;
    let mut cleanup = FileCleanup::new();
    let skipped = report
        .session_conflicts
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let tx = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    for session in &package.sessions {
        if skipped.contains(session.id.as_str()) {
            continue;
        }
        tx.execute(
            "INSERT INTO sessions(id,title,created_at,updated_at) VALUES (?1,?2,?3,?4)",
            params![
                session.id,
                session.title,
                session.created_at,
                session.updated_at
            ],
        )?;
        let final_path = super::event_path(&store.events_dir, &session.id)?;
        if final_path.exists() {
            return Err(BackupError::Invalid(format!(
                "session event path already exists: {}",
                final_path.display()
            )));
        }
        let temp = store
            .events_dir
            .join(format!(".import-{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        cleanup.paths.push(temp.clone());
        let mut offset = 0_i64;
        let mut local_ids = Vec::new();
        for message in package
            .messages
            .iter()
            .filter(|m| m.session_id == session.id)
        {
            tx.execute("INSERT INTO messages(session_id,role,kind,content,metadata,created_at,event_offset,event_length) VALUES (?1,?2,?3,'','null',?4,?5,0)",
                params![session.id, message.role.as_str(), message.kind.as_str(), message.created_at, offset])?;
            let id = tx.last_insert_rowid();
            local_ids.push(id);
            let stored = StoredMessage {
                id,
                session_id: session.id.clone(),
                role: message.role,
                kind: message.kind,
                content: message.content.clone(),
                metadata: message.metadata.clone(),
                created_at: message.created_at,
            };
            let mut bytes = serde_json::to_vec(&stored)?;
            bytes.push(b'\n');
            file.write_all(&bytes)?;
            let length = i64::try_from(bytes.len())
                .map_err(|_| BackupError::Invalid("message too large".into()))?;
            tx.execute(
                "UPDATE messages SET event_length=?2 WHERE id=?1",
                params![id, length],
            )?;
            if message.kind == MessageKind::AgentState {
                tx.execute("INSERT INTO agent_states(message_id,session_id,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
                    params![id, session.id, message.content, serde_json::to_string(&message.metadata)?, message.created_at])?;
            }
            offset = offset
                .checked_add(length)
                .ok_or_else(|| BackupError::Invalid("event stream too large".into()))?;
        }
        file.sync_all()?;
        drop(file);
        if let Some(summary) = package
            .summaries
            .iter()
            .find(|s| s.session_id == session.id)
        {
            let through = if summary.through_ordinal == 0 {
                0
            } else {
                local_ids[summary.through_ordinal - 1]
            };
            tx.execute("INSERT INTO session_summaries(session_id,content,compressed_message_count,updated_at,through_message_id,effective_context) VALUES (?1,?2,?3,?4,?5,?6)",
                params![session.id, summary.content, summary.compressed_message_count, summary.updated_at, through, summary.effective_context])?;
        }
        fs::rename(&temp, &final_path)?;
        cleanup.paths.push(final_path);
    }
    for record in &package.memories {
        let owner = match record.scope {
            MemoryScope::Global => "",
            MemoryScope::Project => target_project_id,
            MemoryScope::Session
                if !skipped.contains(record.owner.as_str())
                    && package.sessions.iter().any(|s| s.id == record.owner) =>
            {
                &record.owner
            }
            MemoryScope::Session => continue,
        };
        let table = if record.scope == MemoryScope::Global {
            "ax_global.scoped_memories"
        } else {
            "scoped_memories"
        };
        tx.execute(&format!("INSERT OR IGNORE INTO {table}(scope,owner,key,value,source,updated_at,always_include) VALUES (?1,?2,?3,?4,?5,?6,?7)"),
            params![record.scope.key(), owner, record.key, record.value, record.source, record.updated_at, record.always_include])?;
    }
    for record in &package.legacy {
        let table = if record.origin == "global" {
            "ax_global.long_term_memory"
        } else {
            "long_term_memory"
        };
        tx.execute(&format!("INSERT OR IGNORE INTO {table}(id,key,value,category,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6)"),
            params![record.id, record.key, record.value, record.category, record.created_at, record.updated_at])?;
    }
    tx.commit()?;
    cleanup.keep = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageRole, NewMessage};

    struct Fixture {
        root: PathBuf,
        project_path: PathBuf,
        global_path: PathBuf,
        project_id: String,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("ax-backup-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            Self {
                project_path: root.join("project.sqlite3"),
                global_path: root.join("global.sqlite3"),
                project_id: Uuid::new_v4().to_string(),
                root,
            }
        }
        fn stores(&self) -> (MemoryStore, MemoryStore) {
            (
                MemoryStore::open(&self.project_path).unwrap(),
                MemoryStore::open(&self.global_path).unwrap(),
            )
        }
        fn seed(&self) -> String {
            let (mut project, global) = self.stores();
            let session = project.create_session("A task").unwrap();
            project
                .append_message(&session.id, NewMessage::text(MessageRole::User, "hello"))
                .unwrap();
            project
                .append_message(
                    &session.id,
                    NewMessage::text(MessageRole::Assistant, "world"),
                )
                .unwrap();
            project
                .save_effective_context(&session.id, "summary", "[]")
                .unwrap();
            project
                .remember_scoped(&MemoryRecord {
                    scope: MemoryScope::Project,
                    owner: self.project_id.clone(),
                    key: "build".into(),
                    value: "cargo test".into(),
                    source: "test".into(),
                    updated_at: 0,
                    always_include: false,
                })
                .unwrap();
            project
                .remember_scoped(&MemoryRecord {
                    scope: MemoryScope::Session,
                    owner: session.id.clone(),
                    key: "mode".into(),
                    value: "brief".into(),
                    source: "test".into(),
                    updated_at: 0,
                    always_include: false,
                })
                .unwrap();
            global
                .remember_scoped(&MemoryRecord {
                    scope: MemoryScope::Global,
                    owner: String::new(),
                    key: "language".into(),
                    value: "Chinese".into(),
                    source: "test".into(),
                    updated_at: 0,
                    always_include: true,
                })
                .unwrap();
            global
                .remember("legacy-key", "legacy-value", "preference")
                .unwrap();
            session.id
        }
        fn export(&self, path: &Path, selection: ExportSelection) -> ImportReport {
            let (project, global) = self.stores();
            ExportService {
                project: &project,
                global: &global,
                project_id: &self.project_id,
            }
            .export(path, selection)
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn round_trip_restores_history_summary_and_all_memory_scopes_with_project_remap() {
        let source = Fixture::new();
        let session_id = source.seed();
        let path = source.root.join("backup.axpack");
        let exported = source.export(&path, ExportSelection::all());
        assert_eq!(
            (
                exported.sessions,
                exported.messages,
                exported.summaries,
                exported.memories,
                exported.legacy_memories
            ),
            (1, 2, 1, 3, 1)
        );
        let target = Fixture::new();
        let target_global = target.root.join("new-home").join("global.sqlite3");
        let preview = ImportService::dry_run(
            &path,
            &target.project_path,
            &target_global,
            Some(&target.project_id),
        )
        .unwrap();
        assert_eq!(preview.sessions, 1);
        assert!(!target.project_path.exists());
        assert!(!target_global.exists());
        let mut project = MemoryStore::open(&target.project_path).unwrap();
        let report = ImportService {
            project: &mut project,
            global_path: &target_global,
            target_project_id: &target.project_id,
        }
        .import(&path)
        .unwrap();
        assert_eq!(preview, report);
        assert_eq!(
            project
                .load_messages(&session_id, None, 10)
                .unwrap()
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["hello", "world"]
        );
        assert_eq!(
            project.session_summary(&session_id).unwrap().as_deref(),
            Some("summary")
        );
        assert_eq!(
            project.effective_context(&session_id).unwrap().as_deref(),
            Some("[]")
        );
        assert_eq!(
            project
                .scoped_memories(MemoryScope::Project, &target.project_id)
                .unwrap()[0]
                .value,
            "cargo test"
        );
        assert!(
            project
                .scoped_memories(MemoryScope::Project, &source.project_id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            project
                .scoped_memories(MemoryScope::Session, &session_id)
                .unwrap()[0]
                .value,
            "brief"
        );
        let global = MemoryStore::open(&target_global).unwrap();
        assert_eq!(
            global.scoped_memories(MemoryScope::Global, "").unwrap()[0].value,
            "Chinese"
        );
        assert_eq!(
            global.recall("legacy-key").unwrap().unwrap().value,
            "legacy-value"
        );
    }

    #[test]
    fn selective_archives_and_conflicts_do_not_replace_current_data() {
        let source = Fixture::new();
        let session_id = source.seed();
        let memory_path = source.root.join("memory.axpack");
        let sessions_path = source.root.join("sessions.axpack");
        source.export(
            &memory_path,
            ExportSelection {
                memory: true,
                sessions: false,
            },
        );
        source.export(
            &sessions_path,
            ExportSelection {
                memory: false,
                sessions: true,
            },
        );
        let memory = read_package(&memory_path).unwrap();
        assert!(
            memory.sessions.is_empty() && memory.messages.is_empty() && !memory.memories.is_empty()
        );
        let sessions = read_package(&sessions_path).unwrap();
        assert!(
            sessions.memories.is_empty()
                && sessions.legacy.is_empty()
                && sessions.sessions.len() == 1
        );
        let target = Fixture::new();
        let (mut project, global) = target.stores();
        project
            .remember_scoped(&MemoryRecord {
                scope: MemoryScope::Project,
                owner: target.project_id.clone(),
                key: "build".into(),
                value: "keep me".into(),
                source: "local".into(),
                updated_at: 0,
                always_include: false,
            })
            .unwrap();
        drop(global);
        let first = ImportService {
            project: &mut project,
            global_path: &target.global_path,
            target_project_id: &target.project_id,
        }
        .import(&sessions_path)
        .unwrap();
        assert_eq!(first.sessions, 1);
        let second = ImportService {
            project: &mut project,
            global_path: &target.global_path,
            target_project_id: &target.project_id,
        }
        .import(&sessions_path)
        .unwrap();
        assert_eq!(second.session_conflicts, vec![session_id]);
        assert_eq!(second.messages, 0);
        let memory_report = ImportService {
            project: &mut project,
            global_path: &target.global_path,
            target_project_id: &target.project_id,
        }
        .import(&memory_path)
        .unwrap();
        assert!(
            memory_report
                .memory_conflicts
                .iter()
                .any(|s| s.contains("build"))
        );
        assert_eq!(memory_report.orphan_session_memories, 1);
        assert_eq!(
            project
                .scoped_memories(MemoryScope::Project, &target.project_id)
                .unwrap()[0]
                .value,
            "keep me"
        );
    }

    #[test]
    fn rejects_old_version_and_corrupt_content_before_database_creation() {
        let source = Fixture::new();
        source.seed();
        let path = source.root.join("backup.axpack");
        source.export(&path, ExportSelection::all());
        let old = source.root.join("old.axpack");
        rewrite_entry(&path, &old, "manifest.json", |bytes| {
            let mut manifest: Value = serde_json::from_slice(bytes).unwrap();
            manifest["version"] = Value::from(0);
            serde_json::to_vec(&manifest).unwrap()
        });
        let target = Fixture::new();
        let error = ImportService::dry_run(&old, &target.project_path, &target.global_path, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported format version"));
        let corrupt = source.root.join("corrupt.axpack");
        rewrite_entry(&path, &corrupt, "memories.jsonl", |_| {
            b"{not JSON}\n".to_vec()
        });
        assert!(
            ImportService::dry_run(&corrupt, &target.project_path, &target.global_path, None)
                .is_err()
        );
        assert!(!target.project_path.exists());
    }

    #[test]
    fn failed_event_install_rolls_back_session_and_memory_inserts() {
        let source = Fixture::new();
        let session_id = source.seed();
        let path = source.root.join("backup.axpack");
        source.export(&path, ExportSelection::all());
        let target = Fixture::new();
        let mut project = MemoryStore::open(&target.project_path).unwrap();
        let block = target
            .root
            .join("sessions")
            .join(format!("{session_id}.jsonl"));
        fs::create_dir_all(&block).unwrap();
        let result = ImportService {
            project: &mut project,
            global_path: &target.global_path,
            target_project_id: &target.project_id,
        }
        .import(&path);
        assert!(result.is_err());
        assert!(project.session(&session_id).unwrap().is_none());
        assert!(
            project
                .scoped_memories(MemoryScope::Project, &target.project_id)
                .unwrap()
                .is_empty()
        );
        let global = MemoryStore::open(&target.global_path).unwrap();
        assert!(
            global
                .scoped_memories(MemoryScope::Global, "")
                .unwrap()
                .is_empty()
        );
    }

    fn rewrite_entry(source: &Path, target: &Path, name: &str, change: impl Fn(&[u8]) -> Vec<u8>) {
        let mut archive = ZipArchive::new(File::open(source).unwrap()).unwrap();
        let mut writer = ZipWriter::new(File::create(target).unwrap());
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let entry_name = entry.name().to_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            writer
                .start_file(&entry_name, FileOptions::default())
                .unwrap();
            let output = if entry_name == name {
                change(&bytes)
            } else {
                bytes
            };
            writer.write_all(&output).unwrap();
        }
        writer.finish().unwrap();
    }
}
