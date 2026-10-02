//! The per-session JSONL event stream and the index that points into it.
//!
//! Raw history is authoritative and append-only; `SQLite` stores byte offsets
//! into the same stream so a resume reads exactly what was written.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use rusqlite::{Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    MemoryError, MemoryStore, MessageKind, StoredMessage,
    message::{RawMessage, decode_message, map_raw_message},
};

impl MemoryStore {
    pub(crate) fn event_path(&self, session_id: &str) -> Result<PathBuf, MemoryError> {
        event_path(&self.events_dir, session_id)
    }

    pub(crate) fn decode_indexed(&self, raw: RawMessage) -> Result<StoredMessage, MemoryError> {
        if let (Some(offset), Some(length)) = (raw.7, raw.8) {
            let mut file = File::open(self.event_path(&raw.1)?)?;
            file.seek(SeekFrom::Start(offset.try_into().map_err(|_| {
                MemoryError::InvalidValue("negative event offset".into())
            })?))?;
            let mut bytes = vec![
                0;
                usize::try_from(length).map_err(|_| MemoryError::InvalidValue(
                    "invalid event length".into()
                ))?
            ];
            file.read_exact(&mut bytes)?;
            let event: StoredMessage = serde_json::from_slice(&bytes)?;
            if event.id != raw.0 || event.session_id != raw.1 {
                return Err(MemoryError::InvalidValue(
                    "event index does not match JSONL".into(),
                ));
            }
            Ok(event)
        } else {
            decode_message(raw)
        }
    }

    pub(crate) fn sync_session(&self, session_id: &str) -> Result<(), MemoryError> {
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)?;
        sync_session_locked(&transaction, &self.events_dir, session_id)?;
        transaction.commit()?;
        Ok(())
    }
}
pub(crate) fn sync_session_locked(
    transaction: &Transaction<'_>,
    events_dir: &Path,
    session_id: &str,
) -> Result<(), MemoryError> {
    let path = event_path(events_dir, session_id)?;
    let (legacy_count, indexed_end): (i64, i64) = transaction.query_row(
        "SELECT COUNT(*) FILTER (WHERE event_offset IS NULL),
                    COALESCE(MAX(event_offset + event_length), 0)
             FROM messages WHERE session_id=?1",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let file_size = match fs::metadata(&path) {
        Ok(meta) => i64::try_from(meta.len())
            .map_err(|_| MemoryError::InvalidValue("event log too large".into()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    if legacy_count == 0 && file_size == indexed_end {
        return Ok(());
    }
    if file_size < indexed_end {
        return Err(MemoryError::InvalidValue(format!(
            "missing JSONL events for session {session_id}"
        )));
    }
    let mut present = HashMap::new();
    if file_size > 0 {
        let mut reader = BufReader::new(File::open(&path)?);
        let mut offset = 0_i64;
        loop {
            let mut line = Vec::new();
            let length = reader.read_until(b'\n', &mut line)?;
            if length == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                return Err(MemoryError::InvalidValue("incomplete JSONL event".into()));
            }
            let event: StoredMessage = serde_json::from_slice(&line)?;
            if event.session_id != session_id {
                return Err(MemoryError::InvalidValue("JSONL session mismatch".into()));
            }
            if present
                .insert(
                    event.id,
                    (offset, i64::try_from(length).unwrap_or(i64::MAX)),
                )
                .is_some()
            {
                return Err(MemoryError::InvalidValue("duplicate JSONL event id".into()));
            }
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1 AND session_id=?2)",
                params![event.id, session_id],
                |row| row.get(0),
            )?;
            if exists {
                transaction.execute("UPDATE messages SET content='',metadata='null',event_offset=?2,event_length=?3 WHERE id=?1",
                        params![event.id, offset, length])?;
            } else {
                transaction.execute("INSERT INTO messages(id,session_id,role,kind,content,metadata,created_at,event_offset,event_length) VALUES (?1,?2,?3,?4,'','null',?5,?6,?7)",
                        params![event.id, session_id, event.role.as_str(), event.kind.as_str(), event.created_at, offset, length])?;
            }
            if event.kind == MessageKind::AgentState {
                transaction.execute("INSERT OR IGNORE INTO agent_states(message_id,session_id,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
                        params![event.id, session_id, event.content, serde_json::to_string(&event.metadata)?, event.created_at])?;
            }
            offset += i64::try_from(length).unwrap_or(i64::MAX);
        }
    }
    let legacy = {
        let mut statement = transaction.prepare("SELECT id,session_id,role,kind,content,metadata,created_at,event_offset,event_length FROM messages WHERE session_id=?1 AND event_offset IS NULL ORDER BY id")?;
        let rows = statement.query_map([session_id], map_raw_message)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for raw in legacy {
        if present.contains_key(&raw.0) {
            continue;
        }
        let event = decode_message(raw)?;
        let (offset, length) = append_event(events_dir, &event)?;
        transaction.execute("UPDATE messages SET content='',metadata='null',event_offset=?2,event_length=?3 WHERE id=?1", params![event.id, offset, length])?;
        if event.kind == MessageKind::AgentState {
            transaction.execute("INSERT OR IGNORE INTO agent_states(message_id,session_id,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
                    params![event.id, session_id, event.content, serde_json::to_string(&event.metadata)?, event.created_at])?;
        }
    }
    Ok(())
}

pub(crate) fn event_path(events_dir: &Path, session_id: &str) -> Result<PathBuf, MemoryError> {
    let id = Uuid::parse_str(session_id)
        .map_err(|_| MemoryError::InvalidValue("invalid session id".into()))?;
    Ok(events_dir.join(format!("{id}.jsonl")))
}

pub(crate) fn append_event(
    events_dir: &Path,
    event: &StoredMessage,
) -> Result<(i64, i64), MemoryError> {
    fs::create_dir_all(events_dir)?;
    let path = event_path(events_dir, &event.session_id)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let offset = i64::try_from(file.metadata()?.len())
        .map_err(|_| MemoryError::InvalidValue("event log too large".into()))?;
    let mut line = serde_json::to_vec(event)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_data()?;
    let length = i64::try_from(line.len())
        .map_err(|_| MemoryError::InvalidValue("event too large".into()))?;
    Ok((offset, length))
}
