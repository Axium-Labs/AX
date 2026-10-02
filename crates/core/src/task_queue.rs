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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedTask {
    pub title: String,
    /// Complete task input; old string queues and checkpoints fall back to title.
    #[serde(default)]
    pub input: String,
    /// Persisted before dispatch so an interrupted task is never silently replaced.
    #[serde(default)]
    pub execution_started: bool,
    #[serde(default)]
    pub depends_on: Vec<usize>,
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
                    input: title.clone(),
                    execution_started: false,
                    title,
                    depends_on: vec![],
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
            QueueState::Active | QueueState::Summarizing | QueueState::Suspended
        ) {
            return;
        }
        self.state = state;
        self.stop_reason = Some(reason);
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
        Message::system(format!("[ax-task-summary]\nSummarize all task outcomes and failures; execution is complete. {}", json!(self.tasks.iter().enumerate().map(|(i,t)| json!({"task":i+1,"status":t.status,"outcome":t.outcome,"failure_reason":t.failure_reason,"child":t.child})).collect::<Vec<_>>())))
    }
}

pub(crate) fn spec() -> ToolSpec {
    ToolSpec { kind: "function", function: FunctionSpec {
        name: TOOL_NAME.into(),
        description: "Create structured tasks only when you explicitly decide decomposition is useful. Lists are formatting hints, never authorization to split. Use alone in a round. execution=controller keeps work in this model; execution=children explicitly delegates complete task inputs to isolated children. dependencies are zero-based prior task indices. Finish the current task with completed/failed/skipped and an outcome. Consider recovery before failure; recovery is advisory. block/cancel stop the goal. Text-only responses never advance tasks.".into(),
        parameters: json!({"type":"object","properties":{
            "action":{"type":"string","enum":["start","finish","block","cancel"]},
            "execution":{"type":"string","enum":["controller","children"]},"dependencies":{"type":"array","items":{"type":"array","items":{"type":"integer","minimum":0}}},
            "overall_goal":{"type":"string"},"tasks":{"type":"array","minItems":2,"items":{"anyOf":[{"type":"string","description":"Complete independently executable task input, not a heading"},{"type":"object","properties":{"title":{"type":"string"},"input":{"type":"string","description":"Complete independently executable task input including necessary data/context"}},"required":["title","input"],"additionalProperties":false}]}},
            "status":{"type":"string","enum":["completed","failed","skipped"]},"reason":{"type":"string"}
        },"required":["action"]}),
    }}
}

fn task_definition(value: &Value) -> Result<(String, String), &str> {
    let (title, task_input) = if let Some(input) = value.as_str() {
        (input, input)
    } else {
        (
            value["title"].as_str().ok_or("task title is required")?,
            value["input"]
                .as_str()
                .ok_or("complete task input is required")?,
        )
    };
    if title.trim().is_empty() || task_input.trim().is_empty() {
        return Err("task title and complete input must be nonempty");
    }
    Ok((title.to_owned(), task_input.to_owned()))
}

pub(crate) fn apply(
    queue: &mut Option<TaskQueue>,
    input: &Value,
    goal_id: &str,
    parent_goal_id: Option<&str>,
) -> Result<(), String> {
    match input["action"].as_str() {
        Some("start") => {
            if queue
                .as_ref()
                .is_some_and(|q| q.goal_id != goal_id || !q.active() || !q.unexecuted())
            {
                return Err("cannot replace a queue after task execution starts; continue it or start a new goal".into());
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
                .map(|(title, _)| title.clone())
                .collect::<Vec<_>>();
            if titles.len() < 2 {
                return Err("at least two tasks required".into());
            }
            let mut plan = TaskQueue::new(goal.into(), titles);
            for (task, (_, input)) in plan.tasks.iter_mut().zip(tasks) {
                task.input = input;
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
                        .any(|(i, deps)| deps.iter().any(|&d| d >= i))
                {
                    return Err("dependencies must refer to prior tasks".into());
                }
                for (task, deps) in plan.tasks.iter_mut().zip(dependencies) {
                    task.depends_on = deps;
                }
            }
            goal_id.clone_into(&mut plan.goal_id);
            plan.parent_goal_id = parent_goal_id.map(str::to_owned);
            *queue = Some(plan);
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
