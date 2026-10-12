//! On-demand document projection and JSONL-first Session handoff.
use crate::repl::ReplState;
use anyhow::{Context, Result, ensure};
use model::Message;
use runtime_core::{
    AgentError, GoalState,
    long_project::{COMMIT_PREFIX, LongProjectConfig, LongProjectHost, ROTATION_PREFIX},
    task_queue::TaskQueue,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub(crate) fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub(crate) fn directory(root: &Path, goal: &str, config: &LongProjectConfig) -> PathBuf {
    let base = config.directory.as_ref().map_or_else(
        || root.join(".ax/long-projects"),
        |p| {
            if p.is_absolute() {
                p.clone()
            } else {
                root.join(p)
            }
        },
    );
    base.join(digest(goal))
}
pub(crate) fn validate(config: &runtime_core::GoalConfig) -> Result<()> {
    if let Some(c) = &config.long_project {
        c.validate().map_err(anyhow::Error::msg)?;
    }
    if let Some(c) = &config.session_rotation {
        c.validate().map_err(anyhow::Error::msg)?;
    }
    Ok(())
}
fn freshness(document: &str, seconds: u64) -> String {
    let stale = seconds > 0
        && document
            .lines()
            .filter_map(|line| line.split("Observed at: ").nth(1))
            .filter_map(|s| s.split_whitespace().next())
            .filter_map(|s| s.trim_end_matches('.').parse::<u64>().ok())
            .any(|t| now().saturating_sub(t) >= seconds);
    if stale {
        "Historical observations have expired; recheck their actual source and environment before reuse.".into()
    } else {
        "Timestamped observations are within policy; untimestamped/manual context still requires source verification.".into()
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Only reference identifiers and safe user/agent prose enter Markdown; never raw tool output.
/// Suspicious lines are omitted in full, including PEM material and credential assignments.
pub(crate) fn safe(value: &str) -> String {
    let mut pem = false;
    value
        .lines()
        .filter_map(|line| {
            let lower = line.to_lowercase();
            if lower.contains("-----begin") {
                pem = true;
                return Some("[sensitive content omitted]".into());
            }
            if pem {
                if lower.contains("-----end") {
                    pem = false;
                }
                return None;
            }
            if [
                "password",
                "passwd",
                "secret",
                "api_key",
                "api-key",
                "apikey",
                "access_token",
                "refresh_token",
                "authorization",
                "bearer ",
                "credential",
                "private key",
                "密钥",
                "凭证",
                "密码",
                "sk-",
                "ghp_",
                "github_pat_",
                "xoxb-",
                "token=",
                "token:",
                "akia",
                "client_secret",
            ]
            .iter()
            .any(|s| lower.contains(s))
            {
                Some("[sensitive content omitted]".into())
            } else if line.split_whitespace().any(|word| {
                word.len() >= 40
                    && word
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_+=/".contains(c))
            }) {
                Some("[opaque sensitive content omitted]".into())
            } else {
                Some(line.replace("<!-- ax:", "&lt;!-- ax:"))
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}
fn regular(path: &Path) -> Result<()> {
    for p in path.ancestors() {
        match fs::symlink_metadata(p) {
            Ok(m) => ensure!(
                !m.file_type().is_symlink(),
                "Long Project paths must not contain symlinks"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub(crate) fn lock(dir: &Path) -> Result<File> {
    regular(dir)?;
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join("document.lock");
    regular(&path)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    fs2::FileExt::try_lock_exclusive(&file)
        .context("Long Project is being updated by another AX; no document was overwritten")?;
    Ok(file)
}
fn read(path: &Path, limit: usize) -> Result<String> {
    regular(path)?;
    match fs::metadata(path) {
        Ok(m) => ensure!(
            m.len() <= limit as u64,
            "Long Project exceeds its configured document limit; archive manual content before continuing"
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e.into()),
    }
    Ok(fs::read_to_string(path)?)
}
fn atomic(path: &Path, content: &str) -> Result<()> {
    regular(path)?;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(e.into());
    }
    #[cfg(unix)]
    {
        File::open(path.parent().context("document parent missing")?)?.sync_all()?;
    }
    Ok(())
}
fn archive(dir: &Path, old: &str) -> Result<()> {
    if old.is_empty() {
        return Ok(());
    }
    let archives = dir.join("archive");
    regular(&archives)?;
    fs::create_dir_all(&archives)?;
    let p = archives.join(format!("{}.md", digest(old)));
    if !p.exists() {
        atomic(&p, &safe(old))?;
    }
    Ok(())
}
fn save(dir: &Path, old: &str, next: &str, limit: usize) -> Result<()> {
    if old == next {
        return Ok(());
    }
    ensure!(
        next.len() <= limit,
        "Long Project update exceeds its document limit; previous document retained"
    );
    let path = dir.join("PROJECT_STATE.md");
    archive(dir, old)?;
    ensure!(
        read(&path, limit)? == old,
        "Long Project changed during update; reload instead of overwriting user edits"
    );
    atomic(&path, next)
}
/// Managed block digests detect human edits. Only intact blocks may be replaced.
fn block(document: &str, key: &str, content: &str) -> String {
    let start = format!("<!-- ax:{key} ");
    let close = format!("<!-- ax:/{key} -->");
    let replacement = format!("{start}{} -->\n{content}\n{close}", digest(content));
    if let Some(a) = document.find(&start)
        && let Some(header_end) = document[a..].find(" -->\n").map(|i| a + i)
        && let Some(b) = document[header_end..].find(&close).map(|i| header_end + i)
    {
        let value = &document[header_end + 5..b];
        let expected = &document[a + start.len()..header_end];
        if digest(value.trim_end_matches('\n')) == expected {
            return format!(
                "{}{}{}",
                &document[..a],
                replacement,
                &document[b + close.len()..]
            );
        }
        // Preserve the edited block byte-for-byte and maintain fresh facts separately.
        return block(document, &format!("{key}-current"), content);
    }
    format!("{}\n\n{replacement}\n", document.trim_end())
}

pub(crate) struct ProjectHost {
    root: PathBuf,
    pub session: Arc<Mutex<String>>,
    owner: String,
    pub database: Mutex<Option<PathBuf>>,
    observed_revision: Mutex<String>,
    queue: Mutex<Option<TaskQueue>>,
    signature: Mutex<String>,
}
impl ProjectHost {
    pub(crate) fn new(root: PathBuf, session: String, owner: String) -> Arc<Self> {
        Arc::new(Self {
            root,
            owner,
            database: Mutex::new(None),
            observed_revision: Mutex::new(String::new()),
            session: Arc::new(Mutex::new(session)),
            queue: Mutex::new(None),
            signature: Mutex::new(String::new()),
        })
    }
    fn document(&self, queue: &TaskQueue, reason: &str, messages: &[Message]) -> Result<String> {
        let goal = queue.goal.as_ref().context("No Goal for Long Project")?;
        let config = goal
            .config
            .long_project
            .as_ref()
            .context("Long Project not configured")?;
        config.validate().map_err(anyhow::Error::msg)?;
        let dir = directory(&self.root, &queue.goal_id, config);
        let _lock = lock(&dir)?;
        let path = dir.join("PROJECT_STATE.md");
        let old = read(&path, config.max_document_bytes)?;
        let mut next = old.clone();
        if next.is_empty() {
            next = if let Some(template) = &config.template {
                safe(template)
                    .replace("{{goal_id}}", &safe(&queue.goal_id))
                    .replace("{{target}}", &safe(&goal.target))
                    .replace("{{acceptance}}", &safe(&goal.config.acceptance.join("; ")))
            } else {
                format!(
                    "# PROJECT_STATE\n\n{}",
                    config
                        .sections
                        .iter()
                        .map(|s| format!("## {s}\n"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            };
        }
        let session = self.session.lock().unwrap().clone();
        let complete: Vec<_> = queue
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.status == runtime_core::task_queue::TaskStatus::Completed)
            .map(|(i, t)| {
                format!(
                    "- task:{i}: {} (checkpoint status: completed; child: {})",
                    safe(&t.title),
                    t.child.as_ref().map_or("none", |c| c.session_id.as_str())
                )
            })
            .rev()
            .take(config.evidence_entries)
            .collect();
        let pending: Vec<_> = queue
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                matches!(
                    t.status,
                    runtime_core::task_queue::TaskStatus::Pending
                        | runtime_core::task_queue::TaskStatus::Running
                )
            })
            .map(|(i, t)| {
                format!(
                    "- task:{i}: {} [{:?}], dependencies: {:?}",
                    safe(&t.title),
                    t.status,
                    t.depends_on
                )
            })
            .take(config.evidence_entries)
            .collect();
        let proofs = goal
            .successful_calls
            .iter()
            .rev()
            .take(config.evidence_entries)
            .map(|id| {
                format!(
                    "- call:{} (successful structured result in Goal checkpoint)",
                    safe(id)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let keeps = |field: &str| config.save_content.iter().any(|s| s == field);
        let mut data = format!(
            "## Runtime checkpoint reference (required)\nGoal: {}\nSession: {session}\nState: {:?}; Queue: {:?}\nTarget: {}\nAcceptance: {}\nSource: Session {session}; [ax-task-queue] checkpoint (authoritative).",
            safe(&queue.goal_id),
            goal.state,
            queue.state,
            safe(&goal.target),
            safe(&goal.config.acceptance.join("; "))
        );
        for (field, value) in [
            (
                "completed_work",
                format!("Completed task evidence:\n{}", complete.join("\n")),
            ),
            (
                "todo_tasks",
                format!(
                    "Unfinished tasks (queue remains authoritative):\n{}",
                    pending.join("\n")
                ),
            ),
            (
                "tool_references",
                format!("Successful call references:\n{proofs}"),
            ),
            (
                "decisions",
                format!("Decision: {}", safe(&format!("{:?}", goal.last_decision))),
            ),
            (
                "issues",
                format!(
                    "Stop/activity: {} / {:?}",
                    safe(goal.stop_reason.as_deref().unwrap_or("")),
                    goal.activity
                ),
            ),
            (
                "environment",
                format!(
                    "Workspace reference: {}\nRefresh environment and source evidence after sleep, interruption or {} seconds; prose never admits completion.",
                    safe(&self.root.display().to_string()),
                    config.stale_after_seconds
                ),
            ),
            (
                "history",
                format!(
                    "History: Session {session}; [ax-task-queue] checkpoint; archive/*.md. Retrieve raw tool receipts on demand."
                ),
            ),
        ] {
            if keeps(field) {
                data.push('\n');
                data.push_str(&value);
            }
        }
        let guidance = if keeps("user_guidance") {
            messages
                .iter()
                .filter(|m| m.role == model::Role::User && m.content != goal.target)
                .map(|m| safe(&m.content))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        };
        let signature = digest(&format!("{data}\n{guidance}"));
        let mut last = self.signature.lock().unwrap();
        let decision_changed = self
            .queue
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|q| q.goal.as_ref())
            .is_some_and(|g| g.last_decision != goal.last_decision);
        let stage_changed = self.queue.lock().unwrap().as_ref().is_some_and(|q| {
            q.tasks.iter().map(|t| t.status).collect::<Vec<_>>()
                != queue.tasks.iter().map(|t| t.status).collect::<Vec<_>>()
        });
        let should_update = config.auto_update
            && (config.update_on.iter().any(|s| s == reason)
                || decision_changed && config.update_on.iter().any(|s| s == "decision")
                || stage_changed && config.update_on.iter().any(|s| s == "stage"));
        if old.is_empty() || reason == "handoff" || should_update && *last != signature {
            next = block(
                &next,
                &format!("runtime-{}", self.owner),
                &format!("{data}\nObserved at: {} (unix seconds)", now()),
            );
            if !guidance.is_empty() {
                next = block(
                    &next,
                    &format!("guidance-{}", self.owner),
                    &format!(
                        "## User guidance\n{guidance}\nSource: user messages in Session {session}; inspect raw history for full instructions."
                    ),
                );
            } else if next.contains(&format!("<!-- ax:guidance-{} ", self.owner)) {
                next = block(&next, &format!("guidance-{}", self.owner), "");
            }
            save(&dir, &old, &next, config.max_document_bytes)?;
            *last = signature;
        }
        *self.observed_revision.lock().unwrap() = digest(&next);
        *self.queue.lock().unwrap() = Some(queue.clone());
        // Surface decision/risk annotations before a possibly long user template.
        let mut notes = Vec::new();
        for fragment in next.split("<!-- ax:note-").skip(1) {
            if let Some((_, body)) = fragment.split_once(" -->\n")
                && let Some((body, _)) = body.split_once("<!-- ax:/note-")
            {
                notes.push(safe(body));
            }
        }
        Ok(format!(
            "Document: {}\nCurrent authoritative Goal state: {:?}. Reconcile Task/Checkpoint state and actual external effects before acting.\nFreshness: {}\nEvidence-linked annotations (verify sources; they never admit completion):\n{}\nDocument excerpt (read long_project or the file for omitted content):\n{}",
            path.display(),
            goal.state,
            freshness(&next, config.stale_after_seconds),
            notes.into_iter().rev().collect::<Vec<_>>().join("\n"),
            safe(&next)
        ))
    }
}
impl LongProjectHost for ProjectHost {
    fn update(
        &self,
        queue: &TaskQueue,
        reason: &str,
        messages: &[Message],
    ) -> std::result::Result<String, AgentError> {
        self.document(queue, reason, messages)
            .map_err(|e| AgentError::GoalStopped {
                state: GoalState::Paused,
                reason: format!("Long Project update failed; working context retained: {e}"),
            })
    }
}

/// Evidence-linked notes use the existing scheduler and filesystem permissions.
pub(crate) struct ProjectTool(pub Arc<ProjectHost>);
#[async_trait::async_trait]
impl tool::Tool for ProjectTool {
    fn name(&self) -> &'static str {
        "long_project"
    }
    fn description(&self) -> &'static str {
        "Maintain concise Goal working context at important decisions/stages. record takes section, text, and sources (successful current Goal call IDs or completed task:N). Notes are evidence-linked annotations, never authoritative completion. Do not store credentials. read retrieves the document and revision; history uses literal query for archives, or session and call_id for recorded successful Goal receipts, with optional before_message_id pagination. Never copy full tool output into the document."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"action":{"type":"string","enum":["record","read","history"]},"section":{"type":"string"},"text":{"type":"string"},"sources":{"type":"array","items":{"type":"string"}},"query":{"type":"string"},"expected_revision":{"type":"string"},"session":{"type":"string"},"call_id":{"type":"string"},"before_message_id":{"type":"integer"}},"required":["action"],"additionalProperties":false})
    }
    fn safety(&self, _: &serde_json::Value) -> tool::SafetyLevel {
        tool::SafetyLevel::Safe
    }
    fn capability(&self, input: &serde_json::Value) -> tool::Capability {
        self.permission(input).capability
    }
    fn permission(&self, input: &serde_json::Value) -> tool::ToolPermission {
        tool::ToolPermission {
            capability: if input["action"] == "record" {
                tool::Capability::FilesystemWrite
            } else {
                tool::Capability::FilesystemRead
            },
            safety: tool::SafetyLevel::Safe,
        }
    }
    fn execution_boundary(&self) -> tool::ExecutionBoundary {
        tool::ExecutionBoundary::RuntimeOwned
    }
    fn runtime_owned_resources(&self) -> bool {
        true
    }
    fn resources(&self, input: &serde_json::Value) -> Vec<tool::ResourceAccess> {
        let queue = self.0.queue.lock().unwrap();
        let dir = queue
            .as_ref()
            .and_then(|q| {
                q.goal
                    .as_ref()
                    .and_then(|g| g.config.long_project.as_ref())
                    .map(|c| directory(&self.0.root, &q.goal_id, c))
            })
            .unwrap_or_else(|| self.0.root.clone());
        let resource = tool::Resource::path(dir);
        let mut resources = vec![if input["action"] == "record" {
            tool::ResourceAccess::write(resource)
        } else {
            tool::ResourceAccess::read(resource)
        }];
        if input["action"] == "history"
            && input["session"].is_string()
            && let Some(database) = self.0.database.lock().unwrap().as_ref()
        {
            resources.push(tool::ResourceAccess::read(tool::Resource::path(database)));
        }
        resources
    }
    async fn execute(
        &self,
        input: serde_json::Value,
    ) -> std::result::Result<String, tool::ToolError> {
        self.apply(&input)
            .map_err(|e| tool::ToolError::Execution(e.to_string()))
    }
}
impl ProjectTool {
    fn session_receipt(
        &self,
        input: &serde_json::Value,
        session: &str,
        queue: &TaskQueue,
        config: &LongProjectConfig,
    ) -> Result<String> {
        uuid::Uuid::parse_str(session)?;
        let call_id = input["call_id"]
            .as_str()
            .context("Session history needs a call_id")?;
        ensure!(
            queue
                .goal
                .as_ref()
                .is_some_and(|goal| goal.successful_calls.contains(call_id)),
            "Call is not recorded as successful in this Goal; unknown effects require explicit reconciliation"
        );
        let database = self
            .0
            .database
            .lock()
            .unwrap()
            .clone()
            .context("Session history repository unavailable")?;
        let store = memory::MemoryStore::open(database)?;
        let state = store
            .latest_agent_state(session, runtime_core::task_queue::STATE_PREFIX)?
            .context("Session has no Goal checkpoint")?;
        let value: serde_json::Value = serde_json::from_str(
            state
                .content
                .strip_prefix(runtime_core::task_queue::STATE_PREFIX)
                .unwrap_or_default(),
        )?;
        ensure!(
            value["goal_id"] == queue.goal_id,
            "Session history belongs to another Goal"
        );
        let mut before = input["before_message_id"].as_i64();
        for _ in 0..config.evidence_entries {
            let page =
                store.load_messages(session, before, crate::session_restore::HISTORY_PAGE_SIZE)?;
            if page.is_empty() {
                return Ok(
                    serde_json::json!({"session":session,"call_id":call_id,"found":false})
                        .to_string(),
                );
            }
            for stored in &page {
                let message = crate::repl::restore_message(stored);
                if message.role == model::Role::Tool
                    && message.tool_call_id.as_deref() == Some(call_id)
                {
                    return Ok(serde_json::json!({"session":session,"message_id":stored.id,"call_id":call_id,"result":message.content,"source":"existing raw Session JSONL evidence; retrieved without executing the tool"}).to_string());
                }
            }
            before = page.first().map(|m| m.id);
        }
        Ok(serde_json::json!({"session":session,"call_id":call_id,"found":false,"next_before_message_id":before}).to_string())
    }

    fn apply(&self, input: &serde_json::Value) -> Result<String> {
        let queue = self
            .0
            .queue
            .lock()
            .unwrap()
            .clone()
            .context("Long Project not initialized")?;
        let goal = queue.goal.as_ref().context("Goal missing")?;
        let config = goal
            .config
            .long_project
            .as_ref()
            .context("Long Project disabled")?;
        let dir = directory(&self.0.root, &queue.goal_id, config);
        let _lock = lock(&dir)?;
        let old = read(&dir.join("PROJECT_STATE.md"), config.max_document_bytes)?;
        match input["action"].as_str() {
            Some("read") => {
                *self.0.observed_revision.lock().unwrap() = digest(&old);
                Ok(serde_json::json!({"revision":digest(&old),"document":safe(&old),"freshness":freshness(&old,config.stale_after_seconds)}).to_string())
            }
            Some("history") => {
                if let Some(session) = input["session"].as_str() {
                    return self.session_receipt(input, session, &queue, config);
                }
                let query = input["query"]
                    .as_str()
                    .context("history needs a literal query")?;
                ensure!(!query.is_empty(), "history query must not be empty");
                let mut found = Vec::new();
                for entry in fs::read_dir(dir.join("archive")).into_iter().flatten() {
                    let p = entry?.path();
                    if p.extension().is_some_and(|s| s == "md") {
                        let text = read(&p, config.max_document_bytes)?;
                        if text.contains(query) {
                            found.push(format!("Source: {}\n{}", p.display(), safe(&text)));
                            break;
                        }
                    }
                }
                Ok(found.join("\n"))
            }
            Some("record") => {
                let observed = self.0.observed_revision.lock().unwrap().clone();
                let expected = input["expected_revision"].as_str().unwrap_or(&observed);
                ensure!(
                    expected == digest(&old),
                    "Long Project revision changed; read current document and merge before recording"
                );
                let section = input["section"].as_str().context("record needs section")?;
                ensure!(
                    config.sections.iter().any(|s| s == section),
                    "section is not configured"
                );
                let text = input["text"].as_str().context("record needs text")?;
                ensure!(
                    !text.trim().is_empty() && text.len() <= config.max_document_bytes / 8,
                    "note is empty or too large"
                );
                ensure!(
                    safe(text) == text,
                    "Sensitive content and managed markers cannot be recorded"
                );
                let sources: Vec<String> = serde_json::from_value(input["sources"].clone())?;
                ensure!(
                    !sources.is_empty() && sources.len() <= config.evidence_entries,
                    "record needs bounded execution evidence references"
                );
                for source in &sources {
                    let known = goal.successful_calls.contains(source)
                        || source
                            .strip_prefix("task:")
                            .and_then(|s| s.parse::<usize>().ok())
                            .and_then(|i| queue.tasks.get(i))
                            .is_some_and(|t| {
                                t.status == runtime_core::task_queue::TaskStatus::Completed
                            });
                    ensure!(known, "Unknown or unsuccessful execution source: {source}");
                }
                let key = format!("note-{}-{}", digest(section), self.0.owner);
                // One concise current note per section; old notes remain in immutable archives.
                let note = format!(
                    "## {}\n{}\nSources: {}\nSession: {}\nObserved at: {}. Agent annotation; validate source and environment before reuse.",
                    safe(section),
                    text,
                    safe(&sources.join(", ")),
                    self.0.session.lock().unwrap(),
                    now()
                );
                let next = block(&old, &key, &note);
                save(&dir, &old, &next, config.max_document_bytes)?;
                *self.0.observed_revision.lock().unwrap() = digest(&next);
                Ok(format!(
                    "Recorded evidence-linked note in {}; revision {}",
                    dir.join("PROJECT_STATE.md").display(),
                    digest(&next)
                ))
            }
            _ => anyhow::bail!("Unknown Long Project action"),
        }
    }
}

/// This runs inside the existing persistence callback, after old history has been flushed.
pub(crate) fn handoff(state: &mut ReplState, marker: &Message) -> Result<()> {
    let data: serde_json::Value = serde_json::from_str(
        marker
            .content
            .strip_prefix(ROTATION_PREFIX)
            .context("handoff marker missing")?,
    )?;
    let context: Vec<Message> = serde_json::from_value(data["context"].clone())?;
    let previous = state.current_session_id()?.to_owned();
    let goal = data["goal_id"]
        .as_str()
        .context("handoff needs Goal identity")?;
    // Publish nothing until all seed messages, context watermark and project binding exist.
    let successor = state
        .store()?
        .create_session(format!("Goal {goal} (rotation)"))?;
    let successor_lease = state.claim_goal_session(&successor.id)?;
    let predecessor = Message::system(format!(
        "[ax-session-predecessor]\n{}",
        serde_json::json!({"previous":previous,"goal_id":goal})
    ));
    append(state, &successor.id, &predecessor)?;
    for message in &context {
        append(state, &successor.id, message)?;
    }
    state
        .store()?
        .save_effective_context(&successor.id, "", &serde_json::to_string(&context)?)?;
    crate::session_projects::register(state.project_location())?;
    let receipt = Message::system(format!(
        "{COMMIT_PREFIX}{}",
        serde_json::json!({"goal_id":goal,"previous":previous,"successor":successor.id,"context_digest":digest(&serde_json::to_string(&context)?)})
    ));
    // Commit belongs to the old append-only log. A crash before commit leaves the old owner authoritative.
    append(state, &previous, &receipt)?;
    state.current_session = Some(successor);
    state.rotation_leases.push(successor_lease);
    Ok(())
}
fn append(state: &mut ReplState, session: &str, message: &Message) -> Result<()> {
    state.store()?.append_message(
        session,
        memory::NewMessage {
            role: crate::repl::memory_role(&message.role),
            kind: if message.role == model::Role::System {
                memory::MessageKind::AgentState
            } else {
                memory::MessageKind::Message
            },
            content: message.content.clone(),
            metadata: serde_json::to_value(message)?,
        },
    )?;
    Ok(())
}

/// Project a committed native lifecycle/configuration change without a model request.
pub(crate) fn refresh_native_state(state: &mut ReplState, queue: &TaskQueue) -> Result<()> {
    if !queue
        .goal
        .as_ref()
        .is_some_and(|g| g.config.long_project.as_ref().is_some_and(|c| c.enabled))
    {
        return Ok(());
    }
    let participant = owner(state)?;
    let host = ProjectHost::new(
        state.project_root.clone(),
        state.current_session_id()?.to_owned(),
        participant,
    );
    host.update(queue, "state", &[])?;
    Ok(())
}

/// Explicit user document edits never change queue execution or permission state.
pub(crate) fn manage(state: &mut ReplState, queue: &TaskQueue, args: &str) -> Result<String> {
    let config = queue
        .goal
        .as_ref()
        .and_then(|g| g.config.long_project.as_ref())
        .context("Enable Long Project with /goal project {\"enabled\":true}")?;
    let dir = directory(&state.project_root, &queue.goal_id, config);
    let _lock = lock(&dir)?;
    let doc = dir.join("PROJECT_STATE.md");
    let old = read(&doc, config.max_document_bytes)?;
    if let Some(path) = args.strip_prefix("export ") {
        let path = state.project_root.join(path.trim());
        regular(&path)?;
        ensure!(
            !path.exists(),
            "Export destination already exists; choose another path"
        );
        atomic(&path, &old)?;
        return Ok(format!("Exported {}", path.display()));
    }
    if let Some(path) = args.strip_prefix("import ") {
        let imported = read(
            &state.project_root.join(path.trim()),
            config.max_document_bytes,
        )?;
        ensure!(!imported.is_empty(), "Import is empty");
        save(&dir, &old, &safe(&imported), config.max_document_bytes)?;
        return Ok(format!(
            "Imported user context into {} (execution state unchanged)",
            doc.display()
        ));
    }
    if let Some(query) = args.strip_prefix("history ") {
        let mut results = Vec::new();
        if let Ok(entries) = fs::read_dir(dir.join("archive")) {
            for entry in entries {
                let p = entry?.path();
                if p.extension().is_some_and(|e| e == "md") {
                    let content = read(&p, config.max_document_bytes)?;
                    if content.contains(query.trim()) {
                        results.push(p.display().to_string());
                    }
                    if results.len() >= config.evidence_entries {
                        break;
                    }
                }
            }
        }
        return Ok(results.join("\n"));
    }
    ensure!(
        args.is_empty() || args == "show",
        "Use project show|import <path>|export <path>|history <query>|<JSON>"
    );
    Ok(format!("{}\n{old}", doc.display()))
}

pub(crate) fn owner(state: &mut ReplState) -> Result<String> {
    let mut session = state.current_session_id()?.to_owned();
    let mut visited = std::collections::HashSet::new();
    loop {
        ensure!(visited.insert(session.clone()), "Session predecessor cycle");
        let Some(marker) = state
            .store()?
            .latest_agent_state(&session, "[ax-session-predecessor]\n")?
        else {
            break;
        };
        let data: serde_json::Value = serde_json::from_str(
            marker
                .content
                .strip_prefix("[ax-session-predecessor]\n")
                .unwrap_or_default(),
        )?;
        data["previous"]
            .as_str()
            .context("predecessor missing")?
            .clone_into(&mut session);
    }
    Ok(digest(&session))
}
/// Existing fenced Workflow carries the working document, alongside its queue checkpoint.
pub(crate) fn distributed_document(
    root: &Path,
    queue: &TaskQueue,
) -> Result<Option<serde_json::Value>> {
    let Some(config) = queue
        .goal
        .as_ref()
        .and_then(|g| g.config.long_project.as_ref())
        .filter(|c| c.enabled)
    else {
        return Ok(None);
    };
    let dir = directory(root, &queue.goal_id, config);
    let content = read(&dir.join("PROJECT_STATE.md"), config.max_document_bytes)?;
    if content.is_empty() {
        return Ok(None);
    }
    let content = safe(&content);
    Ok(Some(
        serde_json::json!({"revision":digest(&content),"content":content}),
    ))
}
pub(crate) fn restore_distributed_document(
    root: &Path,
    queue: &TaskQueue,
    value: &serde_json::Value,
) -> Result<()> {
    let Some(config) = queue
        .goal
        .as_ref()
        .and_then(|g| g.config.long_project.as_ref())
        .filter(|c| c.enabled)
    else {
        return Ok(());
    };
    let document = &value["long_project_document"];
    let content = document["content"].as_str().context(
        "Long Project takeover lacks a working document; inspect host checkpoint before resuming",
    )?;
    ensure!(
        document["revision"].as_str() == Some(digest(content).as_str()),
        "Long Project checkpoint digest mismatch"
    );
    ensure!(
        content.len() <= config.max_document_bytes,
        "Long Project checkpoint exceeds configured size"
    );
    let dir = directory(root, &queue.goal_id, config);
    let _lock = lock(&dir)?;
    let old = read(&dir.join("PROJECT_STATE.md"), config.max_document_bytes)?;
    if old == content {
        return Ok(());
    }
    let next = if old.is_empty() {
        content.to_owned()
    } else {
        block(
            &old,
            "workflow-import",
            &format!(
                "## Fenced Workflow snapshot context\n{}\nSource: workflow generation {}, revision {}. Recheck task ownership and environment before use.",
                safe(content),
                value["generation"],
                document["revision"]
            ),
        )
    };
    save(&dir, &old, &next, config.max_document_bytes)
}

pub(crate) fn merge(base: &mut serde_json::Value, patch: &serde_json::Value) -> Result<()> {
    let object = patch
        .as_object()
        .context("Configuration override needs an object")?;
    for (key, value) in object {
        if value.is_object() && base[key].is_object() {
            merge(&mut base[key], value)?;
        } else {
            base[key] = value.clone();
        }
    }
    Ok(())
}
