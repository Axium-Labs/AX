//! Persistent isolated child sessions, using the existing kernel and memory store.
use async_trait::async_trait;
use memory::{MemoryStore, MessageKind, NewMessage};
use model::Message;
use runtime_core::{
    AgentError, AgentKernel, ChildCheckpoint, ChildHost, ChildOutcome, ChildRun, PreparedChild,
};
use std::path::{Path, PathBuf};

const OUTCOME_PREFIX: &str = "[ax-child-outcome]\n";

pub(crate) struct LocalChildHost {
    pub source: PathBuf,
    pub root: PathBuf,
    pub excluded: Vec<PathBuf>,
}

fn failure(error: impl std::fmt::Display) -> AgentError {
    AgentError::Persistence(error.to_string())
}

fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    let path = path.canonicalize()?;
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(path)
}

fn git(source: &Path, args: &[&std::ffi::OsStr]) -> std::io::Result<std::process::Output> {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(source)
        .args(args)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.output()
}

fn provision_workspace(
    source: &Path,
    destination: &Path,
    child_root: &Path,
    excluded: &[PathBuf],
) -> std::io::Result<()> {
    use std::ffi::OsStr;
    std::fs::create_dir_all(destination.parent().unwrap())?;
    // Keep native Git semantics when available, with a snapshot fallback for
    // non-Git projects or unavailable Git. Existing runtime/scripts are copied.
    let is_repo = git(
        source,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("HEAD"),
        ],
    )
    .is_ok_and(|output| output.status.success());
    if is_repo
        && git(
            source,
            &[
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--detach"),
                destination.as_os_str(),
                OsStr::new("HEAD"),
            ],
        )
        .is_ok_and(|output| output.status.success())
    {
        let deleted = git(
            source,
            &[
                OsStr::new("diff"),
                OsStr::new("--name-only"),
                OsStr::new("--diff-filter=D"),
                OsStr::new("-z"),
                OsStr::new("HEAD"),
            ],
        )?;
        if !deleted.status.success() {
            return Err(std::io::Error::other(
                "cannot snapshot controller deletions",
            ));
        }
        for file in deleted
            .stdout
            .split(|byte| *byte == 0)
            .filter(|file| !file.is_empty())
        {
            let name = std::str::from_utf8(file).map_err(std::io::Error::other)?;
            let path = destination.join(name);
            if path.starts_with(destination) && path.is_file() {
                std::fs::remove_file(path)?;
            }
        }
    }
    copy_workspace(source, destination, child_root, excluded)
}

/// Each snapshot starts from the controller workspace, never a sibling workspace.
fn copy_workspace(
    source: &Path,
    destination: &Path,
    child_root: &Path,
    excluded: &[PathBuf],
) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_name() == ".git" {
            continue;
        }
        if excluded.iter().any(|root| path.starts_with(root))
            || path.starts_with(child_root)
            || entry.file_type()?.is_symlink()
        {
            continue;
        }
        if entry.file_type()?.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(
                    ".git"
                        | ".ax"
                        | ".workbuddy"
                        | "target"
                        | "node_modules"
                        | "release"
                        | "dist"
                        | "build"
                        | ".venv"
                )
            ) {
                continue;
            }
            copy_workspace(
                &path,
                &destination.join(entry.file_name()),
                child_root,
                excluded,
            )?;
        } else {
            std::fs::copy(&path, destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

struct SessionCheckpoint {
    database: PathBuf,
    session: String,
    saved: usize,
}
impl ChildCheckpoint for SessionCheckpoint {
    fn save(&mut self, messages: &[Message]) -> Result<(), AgentError> {
        let mut store = MemoryStore::open(&self.database).map_err(failure)?;
        for message in messages.iter().skip(self.saved) {
            store
                .append_message(
                    &self.session,
                    NewMessage {
                        role: crate::memory_role(&message.role),
                        kind: if message.role == model::Role::System {
                            MessageKind::AgentState
                        } else if message.role == model::Role::Tool
                            || !message.tool_calls.is_empty()
                        {
                            MessageKind::ToolCall
                        } else {
                            MessageKind::Message
                        },
                        content: message.content.clone(),
                        metadata: serde_json::to_value(message).map_err(failure)?,
                    },
                )
                .map_err(failure)?;
            self.saved += 1;
        }
        Ok(())
    }
    fn finish(&mut self, outcome: &ChildOutcome) -> Result<(), AgentError> {
        let mut store = MemoryStore::open(&self.database).map_err(failure)?;
        store
            .append_message(
                &self.session,
                NewMessage {
                    role: memory::MessageRole::System,
                    kind: MessageKind::AgentState,
                    content: format!(
                        "{OUTCOME_PREFIX}{}",
                        serde_json::to_string(outcome).map_err(failure)?
                    ),
                    metadata: serde_json::Value::Null,
                },
            )
            .map_err(failure)?;
        Ok(())
    }
}

#[async_trait]
impl ChildHost for LocalChildHost {
    async fn prepare(
        &self,
        controller: &AgentKernel,
        input: &str,
        previous: Option<&ChildRun>,
    ) -> Result<PreparedChild, AgentError> {
        let (run, database) = if let Some(run) = previous {
            if !run.cwd.is_dir() {
                return Err(AgentError::Tool(tool::ToolError::Execution(
                    "saved child workspace is missing".into(),
                )));
            }
            (run.clone(), run.cwd.join(".ax/child.sqlite3"))
        } else {
            std::fs::create_dir_all(&self.root).map_err(failure)?;
            let root = absolute_path(&self.root).map_err(failure)?;
            let cwd = root
                .join(uuid::Uuid::new_v4().to_string())
                .join("workspace");
            let source = absolute_path(&self.source).map_err(failure)?;
            let excluded = self
                .excluded
                .iter()
                .filter_map(|path| absolute_path(path).ok())
                .collect::<Vec<_>>();
            let destination = cwd.clone();
            tokio::task::spawn_blocking(move || {
                provision_workspace(&source, &destination, &root, &excluded)
            })
            .await
            .map_err(|e| AgentError::WorkerJoin(e.to_string()))?
            .map_err(|e| AgentError::Tool(tool::ToolError::Execution(e.to_string())))?;
            let database = cwd.join(".ax/child.sqlite3");
            std::fs::create_dir_all(database.parent().unwrap()).map_err(failure)?;
            let store = MemoryStore::open(&database).map_err(failure)?;
            let session = store.create_session(input).map_err(failure)?;
            let run = ChildRun {
                goal_id: format!("child-{}", session.id),
                memory_scope: format!("child:{}", session.id),
                execution_budget: Some(controller.child_execution_budget()),
                session_id: session.id,
                cwd,
            };
            (run, database)
        };
        let store = MemoryStore::open(&database).map_err(failure)?;
        if store.session(&run.session_id).map_err(failure)?.is_none() {
            return Err(failure("saved child session is missing"));
        }
        let mut terminal = store
            .latest_agent_state(&run.session_id, OUTCOME_PREFIX)
            .map_err(failure)?
            .map(|message| {
                serde_json::from_str(
                    message
                        .content
                        .strip_prefix(OUTCOME_PREFIX)
                        .unwrap_or_default(),
                )
                .map_err(failure)
            })
            .transpose()?;
        let mut messages = store
            .load_messages(&run.session_id, None, u32::MAX)
            .map_err(failure)?;
        messages.reverse();
        let mut messages = messages
            .into_iter()
            .filter_map(|message| serde_json::from_value::<Message>(message.metadata).ok())
            .filter(|message| !message.content.starts_with("[ax-child-runtime]"))
            .collect::<Vec<_>>();
        // A final assistant message is already durable even if the process
        // disconnected between the final checkpoint and the terminal receipt.
        if terminal.is_none() {
            terminal = runtime_core::child::terminal_outcome(&messages);
        }
        messages.insert(0, Message::system(format!("[ax-child-runtime]\n{}", serde_json::json!({
            "session_id":run.session_id,"cwd":run.cwd,"memory_scope":run.memory_scope,
            "platform":std::env::consts::OS,"shell":if cfg!(windows) { "Windows PowerShell 5.1" } else { "POSIX sh" }
        }))));
        let kernel = controller.fork_child(run.clone(), input, messages);
        Ok(PreparedChild {
            checkpoint: Box::new(SessionCheckpoint {
                database,
                session: run.session_id.clone(),
                saved: 0,
            }),
            run,
            kernel,
            terminal,
        })
    }
}

#[cfg(test)]
#[path = "child_runtime_tests.rs"]
mod tests;
