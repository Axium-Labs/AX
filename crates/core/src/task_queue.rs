//! Lightweight turn orchestration; tool execution remains in the existing DAG.
use model::{FunctionSpec, Message, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const ARCHIVE_PREFIX: &str = "[ax-task-queue-archive]\n";
pub const STATE_PREFIX: &str = "[ax-task-queue]\n";
pub(crate) const PROGRESS_PREFIX: &str = "[ax-progress]\n";
pub(crate) const TOOL_NAME: &str = "task_queue";

/// Explicit caller intent; loading a session never implies resuming its goal.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum GoalTurn {
    #[default]
    New,
    Start {
        goal_id: String,
    },
    Resume {
        goal_id: String,
    },
    /// Resume a run that suspended on `request_user_input`, writing the answer
    /// back to the tool call that asked.
    Answer {
        goal_id: String,
        answer: crate::UserAnswer,
    },
    Cancel {
        goal_id: String,
    },
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    #[default]
    Active,
    Summarizing,
    Suspended,
    /// Parked on a `request_user_input` question. Resumable, never failed.
    WaitingForUser,
    Completed,
    Blocked,
    Cancelled,
    Superseded,
}

pub(crate) fn fresh_goal_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "goal-{:x}-{nanos:x}-{:x}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

/// Task-owned workspace prepared by the host before any child model request.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSpec {
    #[serde(default)]
    pub mode: WorkspaceMode,
    pub repo_url: Option<String>,
    pub revision: Option<String>,
    pub subdir: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    #[default]
    Inherit,
    Git,
    Empty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedTask {
    #[serde(default)]
    pub workspace: WorkspaceSpec,
    #[serde(default)]
    pub output_dir: Option<String>,
    pub title: String,
    /// Complete task input; old string queues and checkpoints fall back to title.
    #[serde(default)]
    pub input: String,
    /// Persisted before dispatch so an interrupted task is never silently replaced.
    #[serde(default)]
    pub execution_started: bool,
    #[serde(default)]
    pub depends_on: Vec<usize>,
    /// Resources this task touches. The runtime refuses to run two tasks whose
    /// declared accesses conflict (write/write, or read against write), so an
    /// opaque task never races a declared one on the same path.
    #[serde(default)]
    pub resources: Vec<tool::ResourceAccess>,
    pub status: TaskStatus,
    pub failure_reason: Option<String>,
    pub outcome: Option<String>,
    #[serde(default)]
    pub recovery_attempts: usize,
    #[serde(default)]
    pub child: Option<crate::ChildRun>,
}

impl QueuedTask {
    #[must_use]
    pub fn task_input(&self) -> &str {
        if self.input.is_empty() {
            &self.title
        } else {
            &self.input
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskQueue {
    #[serde(default)]
    pub goal_id: String,
    #[serde(default)]
    pub parent_goal_id: Option<String>,
    #[serde(default)]
    pub state: QueueState,
    #[serde(default)]
    pub final_response: Option<String>,
    pub overall_goal: String,
    pub tasks: Vec<QueuedTask>,
    #[serde(default)]
    pub delegate: bool,
    pub summarized: bool,
    pub stop_reason: Option<String>,
}

impl TaskQueue {
    pub(crate) fn new(goal: String, titles: Vec<String>) -> Self {
        let mut queue = Self {
            goal_id: fresh_goal_id(),
            parent_goal_id: None,
            state: QueueState::Active,
            final_response: None,
            overall_goal: goal,
            tasks: titles
                .into_iter()
                .map(|title| QueuedTask {
                    workspace: WorkspaceSpec::default(),
                    output_dir: None,
                    input: title.clone(),
                    execution_started: false,
                    title,
                    depends_on: vec![],
                    resources: vec![],
                    status: TaskStatus::Pending,
                    failure_reason: None,
                    outcome: None,
                    recovery_attempts: 0,
                    child: None,
                })
                .collect(),
            delegate: false,
            summarized: false,
            stop_reason: None,
        };
        queue.advance();
        queue
    }

    /// Formatting hints from a user message, used by tests that need a model to
    /// "read" a list. No runtime path consults this: a queue exists only after an
    /// explicit `task_queue` call, so list formatting never creates tasks,
    /// workers or approval.
    #[must_use]
    pub fn list_hints(input: &str) -> Vec<String> {
        let mut titles: Vec<String> = Vec::new();
        let mut fenced = false;
        for line in input.lines() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            let trimmed = line.trim_start();
            let number_len = trimmed.bytes().take_while(u8::is_ascii_digit).count();
            let marker = trimmed[number_len..].chars().next();
            let numbered = !fenced
                && line == trimmed
                && number_len > 0
                && matches!(marker, Some('.' | ')' | '、'))
                && trimmed[..number_len].parse::<usize>().ok() == Some(titles.len() + 1);
            let bullet = !fenced
                && line == trimmed
                && (trimmed.starts_with("- ") || trimmed.starts_with("* "));
            if numbered || bullet {
                let offset = if bullet {
                    2
                } else {
                    number_len + marker.map_or(0, char::len_utf8)
                };
                let title = trimmed[offset..].trim();
                if !title.is_empty() {
                    titles.push(title.to_owned());
                }
            } else if let Some(title) = titles.last_mut() {
                title.push('\n');
                title.push_str(line);
            }
        }
        titles
    }

    fn unexecuted(&self) -> bool {
        self.tasks.iter().all(|task| {
            !task.execution_started
                && task.child.is_none()
                && matches!(task.status, TaskStatus::Pending | TaskStatus::Running)
                && task.outcome.is_none()
                && task.failure_reason.is_none()
                && task.recovery_attempts == 0
        })
    }

    pub(crate) fn active(&self) -> bool {
        matches!(self.state, QueueState::Active | QueueState::Summarizing)
    }
    pub(crate) fn current_mut(&mut self) -> Option<&mut QueuedTask> {
        if self.state != QueueState::Active {
            return None;
        }
        self.tasks
            .iter_mut()
            .find(|task| task.status == TaskStatus::Running)
    }
    pub(crate) fn advance(&mut self) {
        if self.state != QueueState::Active {
            return;
        }
        for index in 0..self.tasks.len() {
            if self.tasks[index].status == TaskStatus::Pending
                && self.tasks[index].depends_on.iter().any(|&dep| {
                    self.tasks.get(dep).is_none_or(|task| {
                        matches!(task.status, TaskStatus::Failed | TaskStatus::Skipped)
                    })
                })
            {
                self.tasks[index].status = TaskStatus::Skipped;
                self.tasks[index].failure_reason = Some("dependency failed or unavailable".into());
            }
        }
        if !self.tasks.iter().any(|t| t.status == TaskStatus::Running) {
            let ready = self.tasks.iter().position(|t| {
                t.status == TaskStatus::Pending
                    && t.depends_on.iter().all(|&dep| {
                        self.tasks
                            .get(dep)
                            .is_some_and(|d| d.status == TaskStatus::Completed)
                    })
            });
            if let Some(index) = ready {
                self.tasks[index].status = TaskStatus::Running;
            }
        }
        if !self.tasks.is_empty()
            && !self
                .tasks
                .iter()
                .any(|task| matches!(task.status, TaskStatus::Pending | TaskStatus::Running))
        {
            self.state = QueueState::Summarizing;
        }
    }

    pub fn stop(&mut self, state: QueueState, reason: String) {
        if !matches!(
            self.state,
            QueueState::Active
                | QueueState::Summarizing
                | QueueState::Suspended
                | QueueState::WaitingForUser
        ) {
            return;
        }
        self.state = state;
        self.stop_reason = Some(reason);
    }

    /// Park the goal on a `request_user_input` question. Suspension is
    /// resumable: it is never reported as failure and never drops the queue.
    pub(crate) fn await_user(&mut self) {
        if matches!(self.state, QueueState::Active | QueueState::Summarizing) {
            self.state = QueueState::WaitingForUser;
        }
    }

    /// A goal that still has work to resume, including one waiting on a user.
    #[must_use]
    pub(crate) fn resumable(&self) -> bool {
        matches!(
            self.state,
            QueueState::Active
                | QueueState::Summarizing
                | QueueState::Suspended
                | QueueState::WaitingForUser
        )
    }

    pub fn normalize_legacy(&mut self) {
        // Saved child receipts prove a previous explicit delegation; resume it.
        if self.tasks.iter().any(|t| t.child.is_some()) {
            self.delegate = true;
        }
        if self.goal_id.is_empty() {
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            self.overall_goal.hash(&mut hash);
            for task in &self.tasks {
                task.title.hash(&mut hash);
            }
            self.goal_id = format!("legacy-{:x}", hash.finish());
        }
        if self.state == QueueState::Active {
            if self.summarized {
                self.state = QueueState::Completed;
            } else if self.stop_reason.is_some() {
                self.state = QueueState::Cancelled;
            } else {
                self.advance();
            }
        }
    }

    pub(crate) fn finish(&mut self, status: TaskStatus, reason: String) -> Result<(), String> {
        let task = self.current_mut().ok_or("no running task")?;
        if !matches!(
            status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Skipped
        ) {
            return Err("finish requires a terminal status".into());
        }
        if status != TaskStatus::Completed && reason.trim().is_empty() {
            return Err("failed/skipped tasks require a reason".into());
        }
        task.status = status;
        task.outcome = Some(reason.clone());
        task.failure_reason = (status != TaskStatus::Completed).then_some(reason);
        self.advance();
        Ok(())
    }
    pub(crate) fn progress(&self) -> Message {
        let current = self
            .tasks
            .iter()
            .find(|task| task.status == TaskStatus::Running);
        Message::system(format!(
            "{PROGRESS_PREFIX}{}",
            json!({
                "overall_goal": self.overall_goal,
                "current_task": current.map(|task| &task.title),
                "current_task_input": current.map(QueuedTask::task_input),
                "completed_count": self.tasks.iter().filter(|task| task.status == TaskStatus::Completed).count(),
                "failed_count": self.tasks.iter().filter(|task| task.status == TaskStatus::Failed).count(),
                "remaining_tasks": self.tasks.iter().enumerate().filter(|(_,task)| task.status == TaskStatus::Pending).map(|(i,_)| i+1).collect::<Vec<_>>()
            })
        ))
    }
    #[must_use]
    pub fn snapshot(&self) -> Message {
        let mut state = json!(self);
        state["remaining_tasks"] = json!(
            self.tasks
                .iter()
                .enumerate()
                .filter(|(_, task)| matches!(
                    task.status,
                    TaskStatus::Pending | TaskStatus::Running
                ))
                .map(|(index, _)| index + 1)
                .collect::<Vec<_>>()
        );
        Message::system(format!("{STATE_PREFIX}{state}"))
    }
    pub(crate) fn restore(messages: &mut Vec<Message>) -> Option<Self> {
        let mut queue: Option<Self> = messages
            .iter()
            .rev()
            .filter(|m| m.role == model::Role::System)
            .find_map(|m| {
                m.content
                    .strip_prefix(STATE_PREFIX)
                    .and_then(|v| serde_json::from_str(v).ok())
            });
        messages.retain(|m| {
            m.role != model::Role::System
                || (!m.content.starts_with(STATE_PREFIX)
                    && !m.content.starts_with(ARCHIVE_PREFIX)
                    && !m.content.starts_with(PROGRESS_PREFIX)
                    && !m.content.starts_with("[ax-recovery]"))
        });
        if let Some(queue) = &mut queue {
            queue.normalize_legacy();
        }
        queue
    }
    pub(crate) fn summary_context(&self) -> Message {
        Message::system(format!(
            "[ax-task-summary]\nKnown execution items are terminal, including evidenced failures. Complete any remaining requested durable reports before final. Use child_result or child_result.json for authoritative current receipts when available: aggregate their terminal status, never custom result.json status labels. Read artifact-manifest.json to identify files exported by this run; files absent from it may be stale and must not supply current evaluation. Use measured metrics.json (wall_time_ms is the measured execution duration); absent measurements and unknown error/evaluation counts stay null, never zero. Verify reports and the final answer agree with this inventory. Check every requested per-item output for both completed and failed items. Produce missing requested report files from current receipts with explicit unknown/null values; listing missing deliverables is not completing them. Create cross-item reports in the controller; an isolated child needs complete report inputs and cannot infer access to sibling output directories. Do not confuse failed items with never-dispatched items. {}",
            json!({"total_tasks":self.tasks.len(),"overall_goal":self.overall_goal,"tasks":self.tasks.iter().enumerate().map(|(i,t)| json!({"task":i+1,"task_id":format!("task-{}",i+1),"title":t.title,"status":t.status,"outcome":t.outcome,"failure_reason":t.failure_reason,"workspace":t.workspace,"output_dir":t.output_dir,"child":t.child})).collect::<Vec<_>>() })
        ))
    }
}

pub(crate) fn spec() -> ToolSpec {
    ToolSpec { kind: "function", function: FunctionSpec {
        name: TOOL_NAME.into(),
        description: "Do ordinary setup/data reads before creating a queue. Queue concrete user deliverables, never a discovery checklist. Prefer task_source work mapping for projected tables. Register concrete executable work once known; append newly discovered items without replacing executed work. Prefer execution=children for independent work. Lists of instructions are not tasks. workspace declares repo_url and exact revision; output_dir preserves child artifacts before cleanup. Use alone in a round. execution=children delegates complete task inputs to isolated children; the runtime dispatches EVERY independent task at once, bounded by its own concurrency limit, and keeps dependent tasks waiting for their predecessor. dependencies are zero-based prior task indices. Declare resources (path or name, optional write flag) so two tasks that write the same thing never run at the same time. Finish the current task with completed/failed/skipped and an outcome. Consider recovery before failure; recovery is advisory. cancel stops the goal; actual runtime global blockers are classified by the controller. Text-only responses never advance tasks.".into(),
        parameters: json!({"type":"object","properties":{
            "action":{"type":"string","enum":["start","append","finish","block","cancel"]},
            "execution":{"type":"string","enum":["controller","children"]},"dependencies":{"type":"array","items":{"type":"array","items":{"type":"integer","minimum":0}}},
            "overall_goal":{"type":"string"},"tasks":{"type":"array","minItems":2,"items":{"anyOf":[{"type":"string","description":"Complete independently executable task input, not a heading"},{"type":"object","properties":{"title":{"type":"string"},"input":{"type":"string","description":"Complete independently executable task input including necessary data/context"},"workspace":{"type":"object","properties":{"mode":{"type":"string","enum":["inherit","git","empty"]},"repo_url":{"type":"string"},"revision":{"type":"string"},"subdir":{"type":"string"}},"required":["mode"],"additionalProperties":false},"output_dir":{"type":"string"},"resources":{"type":"array","description":"Files or named resources this task touches","items":{"anyOf":[{"type":"string","description":"Path read by this task"},{"type":"object","properties":{"path":{"type":"string"},"name":{"type":"string"},"all":{"type":"boolean"},"write":{"type":"boolean","description":"True when the task modifies it"}},"additionalProperties":false}]}}},"required":["title","input"],"additionalProperties":false}]}},
            "evidence_call_ids":{"type":"array","items":{"type":"string"}},"status":{"type":"string","enum":["completed","failed","skipped"]},"reason":{"type":"string"}
        },"required":["action"]}),
    }}
}

fn task_definition(value: &Value) -> Result<(String, String, Vec<tool::ResourceAccess>), String> {
    let (title, task_input, resources) = if let Some(input) = value.as_str() {
        (input, input, Vec::new())
    } else {
        (
            value["title"].as_str().ok_or("task title is required")?,
            value["input"]
                .as_str()
                .ok_or("complete task input is required")?,
            match value.get("resources") {
                Some(resources) => parse_resources(resources)?,
                None => Vec::new(),
            },
        )
    };
    if title.trim().is_empty() || task_input.trim().is_empty() {
        return Err("task title and complete input must be nonempty".into());
    }
    Ok((title.to_owned(), task_input.to_owned(), resources))
}

/// Declared task resources. A bare string is a read of that path; an object
/// carries `path` (or `name`) plus an optional `write` flag.
fn parse_resources(value: &Value) -> Result<Vec<tool::ResourceAccess>, String> {
    let entries = value.as_array().ok_or("resources must be an array")?;
    let mut parsed = Vec::with_capacity(entries.len());
    for entry in entries {
        if let Some(path) = entry.as_str().filter(|path| !path.is_empty()) {
            parsed.push(tool::ResourceAccess::read(tool::Resource::path(path)));
            continue;
        }
        let write = entry["write"].as_bool().unwrap_or(false);
        let resource = if let Some(path) = entry["path"].as_str().filter(|path| !path.is_empty()) {
            tool::Resource::path(path)
        } else if let Some(name) = entry["name"].as_str().filter(|name| !name.is_empty()) {
            tool::Resource::Named(name.to_owned())
        } else if entry["all"].as_bool() == Some(true) {
            tool::Resource::All
        } else {
            return Err("each resource needs path, name or all".into());
        };
        parsed.push(tool::ResourceAccess { resource, write });
    }
    Ok(parsed)
}

// One admission transaction retains task IDs, history and dependency offsets.
#[allow(clippy::too_many_lines)]
pub(crate) fn apply(
    queue: &mut Option<TaskQueue>,
    input: &Value,
    goal_id: &str,
    parent_goal_id: Option<&str>,
) -> Result<(), String> {
    match input["action"].as_str() {
        Some("start" | "append") => {
            let append = input["action"] == "append";
            if queue.as_ref().is_some_and(|q| {
                q.goal_id != goal_id || !q.active() || (!append && !q.unexecuted())
            }) {
                return Err("cannot replace executed work; use action=append for newly discovered concrete items, finish current work or start a new goal".into());
            }
            let goal = input["overall_goal"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or("overall_goal is required")?;
            let tasks = input["tasks"]
                .as_array()
                .ok_or("tasks are required")?
                .iter()
                .map(task_definition)
                .collect::<Result<Vec<_>, _>>()?;
            let titles = tasks
                .iter()
                .map(|(title, _, _)| title.clone())
                .collect::<Vec<_>>();
            if titles.len() < if append { 1 } else { 2 } {
                return Err("at least two tasks required".into());
            }
            let offset = if append {
                queue.as_ref().map_or(0, |queue| queue.tasks.len())
            } else {
                0
            };
            let mut plan = TaskQueue::new(goal.into(), titles);
            for ((task, (_, input, resources)), definition) in plan
                .tasks
                .iter_mut()
                .zip(tasks)
                .zip(input["tasks"].as_array().unwrap())
            {
                if let Some(workspace) = definition.get("workspace") {
                    task.workspace = serde_json::from_value(workspace.clone())
                        .map_err(|e| format!("invalid workspace: {e}"))?;
                    validate_workspace(&task.workspace)?;
                }
                task.output_dir = definition["output_dir"].as_str().map(str::to_owned);
                task.input = input;
                task.resources = resources;
            }
            if let Some(mode) = input.get("execution") {
                if !matches!(mode.as_str(), Some("controller" | "children")) {
                    return Err("invalid execution mode".into());
                }
                plan.delegate = mode == "children";
            }
            if let Some(dependencies) = input.get("dependencies") {
                let dependencies: Vec<Vec<usize>> = serde_json::from_value(dependencies.clone())
                    .map_err(|_| "invalid dependencies")?;
                if dependencies.len() != plan.tasks.len()
                    || dependencies
                        .iter()
                        .enumerate()
                        .any(|(i, deps)| deps.iter().any(|&d| d >= offset + i))
                {
                    return Err("dependencies must refer to prior tasks".into());
                }
                for (task, deps) in plan.tasks.iter_mut().zip(dependencies) {
                    task.depends_on = deps;
                }
            }
            goal_id.clone_into(&mut plan.goal_id);
            plan.parent_goal_id = parent_goal_id.map(str::to_owned);
            if append && let Some(existing) = queue.as_mut() {
                for task in &mut plan.tasks {
                    task.status = TaskStatus::Pending;
                }
                existing.tasks.extend(plan.tasks);
                if input.get("execution").is_some() {
                    existing.delegate = plan.delegate;
                }
                existing.state = QueueState::Active;
                existing.summarized = false;
                existing.advance();
            } else {
                *queue = Some(plan);
            }
            Ok(())
        }
        Some("finish") => {
            let status =
                serde_json::from_value(input["status"].clone()).map_err(|_| "invalid status")?;
            let queue = queue
                .as_mut()
                .filter(|q| q.goal_id == goal_id && q.active())
                .ok_or("no active goal")?;
            if queue.state == QueueState::Summarizing {
                return Err("goal is summarizing; no task finish is required".into());
            }
            queue.finish(status, input["reason"].as_str().unwrap_or("").into())
        }
        Some("block" | "cancel") => {
            let reason = input["reason"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or("stop reason required")?;
            let queue = queue
                .as_mut()
                .filter(|q| q.goal_id == goal_id && q.active())
                .ok_or("no active goal")?;
            let state = if input["action"] == "block" {
                QueueState::Blocked
            } else {
                QueueState::Cancelled
            };
            queue.stop(state, reason.into());
            Ok(())
        }
        _ => Err("unknown task_queue action".into()),
    }
}

pub(crate) fn schema_tokens() -> usize {
    let spec = spec();
    crate::token::estimate_text_tokens(&spec.function.name)
        + crate::token::estimate_text_tokens(&spec.function.description)
        + crate::token::estimate_text_tokens(&spec.function.parameters.to_string())
}

pub(crate) fn validate_workspace(spec: &WorkspaceSpec) -> Result<(), String> {
    if spec.mode == WorkspaceMode::Git
        && spec
            .repo_url
            .as_deref()
            .is_none_or(|url| url.trim().is_empty() || url.starts_with('-'))
    {
        return Err("git workspace requires a non-option repo_url".into());
    }
    if let Some(subdir) = &spec.subdir {
        let path = std::path::Path::new(subdir);
        if path.is_absolute()
            || path.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            })
            || subdir.contains('\\') && subdir.split('\\').any(|part| part == "..")
        {
            return Err("workspace subdir must stay within workspace".into());
        }
    }
    Ok(())
}
