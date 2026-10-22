//! Public delegated activity and trusted child-history recovery for ACP.
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use anyhow::Result;
use memory::{MemoryStore, MessageKind, MessageRole, NewMessage};
use runtime_core::{ChildRun, SubagentActivity};
use serde_json::{Value, json};

use super::{Outbox, event_update, stamp};

pub(super) const PREFIX: &str = runtime_core::subagent::ACTIVITY_PREFIX;

pub(super) fn activity_update(
    activity: &SubagentActivity,
    _calls: &mut HashMap<String, String>,
) -> Value {
    let mut body = json!({"sessionUpdate":"subagent","agentId":activity.id,
        "parentAgentId":activity.parent_id,"task":activity.task,
        "title":activity.task.lines().next().unwrap_or_default().chars().take(80).collect::<String>(),
        "status":activity.status});
    if let Some(run) = &activity.run {
        body["childSessionId"] = json!(run.session_id);
    }
    if let Some(summary) = &activity.summary {
        body["summary"] = json!(summary);
    }
    if let Some(error) = &activity.error {
        body["error"] = json!(error);
    }
    if let Some(event) = &activity.event
        // The envelope scopes tool identities. Child calls never enter the
        // controller's or a sibling's call-id bookkeeping.
        && let Some(update) = event_update((**event).clone(), &mut HashMap::new())
    {
        body["update"] = update;
    }
    body
}

pub(super) fn persist(
    store: &mut Option<MemoryStore>,
    database: &Path,
    session: &str,
    activity: &SubagentActivity,
) -> Result<()> {
    let mut body = activity_update(activity, &mut HashMap::new());
    body["run"] = json!(activity.run);
    let store = match store {
        Some(store) => store,
        slot @ None => slot.insert(MemoryStore::open(database)?),
    };
    store.append_message(
        session,
        NewMessage {
            role: MessageRole::System,
            kind: MessageKind::AgentState,
            content: format!("{PREFIX}{}", serde_json::to_string(&body)?),
            metadata: Value::Null,
        },
    )?;
    Ok(())
}

pub(super) struct HistoryReplay<'a> {
    pub out: &'a Outbox,
    pub session: &'a str,
    /// All descriptors must resolve beneath this AX-owned child root.
    pub root: &'a Path,
    pub seen: HashSet<String>,
    bytes: usize,
    updates: usize,
}

impl HistoryReplay<'_> {
    pub fn new<'a>(out: &'a Outbox, session: &'a str, root: &'a Path) -> HistoryReplay<'a> {
        HistoryReplay {
            out,
            session,
            root,
            seen: HashSet::new(),
            bytes: 32 * 1024 * 1024,
            updates: 20_000,
        }
    }

    pub fn send(&mut self, body: Value, at: i64, scope: Option<&Value>) {
        let body = if let Some(scope) = scope {
            if self.updates == 0 {
                return;
            }
            self.updates -= 1;
            let mut wrapped = scope.clone();
            wrapped["update"] = body;
            wrapped
        } else {
            body
        };
        let _ = self
            .out
            .send(json!({"jsonrpc":"2.0","method":"session/update",
            "params":{"sessionId":self.session,"update":stamp(body,at)}}));
    }

    pub fn child(&mut self, mut body: Value, at: i64, parent: Option<&Value>) {
        let run = body
            .get("run")
            .cloned()
            .and_then(|run| serde_json::from_value::<ChildRun>(run).ok());
        if let Some(object) = body.as_object_mut() {
            object.remove("run");
        }
        if body.get("parentAgentId").is_none_or(Value::is_null)
            && let Some(parent) = parent
        {
            body["parentAgentId"] = parent["agentId"].clone();
        }
        self.send(body.clone(), at, None);
        let Some(run) = run else {
            return;
        };
        if body["childSessionId"].as_str() != Some(&run.session_id) {
            return;
        }
        // Cycles and duplicate checkpoints cannot recursively expand history.
        if !self.seen.insert(run.session_id.clone()) {
            return;
        }
        if self.seen.len() > 256 || self.bytes == 0 || self.updates == 0 {
            self.truncated(body, at);
            return;
        }
        let Some(path) = child_events(self.root, &run) else {
            return;
        };
        // Read authoritative JSONL only. Inspecting a child never creates a
        // database, applies migrations or repairs its index.
        let Ok(file) = File::open(path) else {
            return;
        };
        let file_bytes = file.metadata().map_or(0, |metadata| metadata.len());
        let mut reader = BufReader::new(file);
        let mut messages = Vec::new();
        let mut consumed = 0;
        while self.bytes > 0 {
            let mut line = Vec::new();
            let Ok(count) = Read::take(&mut reader, self.bytes as u64).read_until(b'\n', &mut line)
            else {
                break;
            };
            if count == 0 {
                break;
            }
            self.bytes -= count;
            consumed += count as u64;
            if let Ok(message) = serde_json::from_slice::<memory::StoredMessage>(&line)
                && message.session_id == run.session_id
            {
                messages.push(message);
            }
        }
        super::replay_messages(self, messages, Some(&body));
        if consumed < file_bytes || self.updates == 0 {
            self.truncated(body, at);
        }
    }

    fn truncated(&mut self, mut body: Value, at: i64) {
        body["historyTruncated"] = json!(true);
        self.send(body, at, None);
    }
}

fn child_events(root: &Path, run: &ChildRun) -> Option<PathBuf> {
    uuid::Uuid::parse_str(&run.session_id).ok()?;
    let root = root.canonicalize().ok()?;
    let state = run.state_dir.as_ref()?.canonicalize().ok()?;
    if !state.starts_with(&root) || state.file_name()? != "state" {
        return None;
    }
    let events = state
        .join("sessions")
        .join(format!("{}.jsonl", run.session_id))
        .canonicalize()
        .ok()?;
    (events.starts_with(&state) && events.is_file()).then_some(events)
}

pub(super) fn outcome(body: &Value, result: &Value) -> Value {
    let mut body = body.clone();
    body["status"] = json!(match result["status"].as_str() {
        Some("completed") => "completed",
        Some("cancelled") => "cancelled",
        _ => "failed",
    });
    body["summary"] = result["summary"].clone();
    body["error"] = result["failure_reason"].clone();
    body
}
