//! Lightweight turn orchestration; tool execution remains in the existing DAG.
use model::{FunctionSpec, Message, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const STATE_PREFIX: &str = "[ax-task-queue]\n";
pub(crate) const PROGRESS_PREFIX: &str = "[ax-progress]\n";
pub(crate) const TOOL_NAME: &str = "task_queue";

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
    pub status: TaskStatus,
    pub failure_reason: Option<String>,
    pub outcome: Option<String>,
    #[serde(default)]
    pub recovery_attempts: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskQueue {
    pub overall_goal: String,
    pub tasks: Vec<QueuedTask>,
    pub summarized: bool,
    pub stop_reason: Option<String>,
}

impl TaskQueue {
    pub(crate) fn new(goal: String, titles: Vec<String>) -> Self {
        let mut queue = Self {
            overall_goal: goal,
            tasks: titles
                .into_iter()
                .map(|title| QueuedTask {
                    title,
                    status: TaskStatus::Pending,
                    failure_reason: None,
                    outcome: None,
                    recovery_attempts: 0,
                })
                .collect(),
            summarized: false,
            stop_reason: None,
        };
        queue.advance();
        queue
    }

    /// Recognize explicit top-level numbered task lists without another model call.
    pub(crate) fn from_input(input: &str) -> Option<Self> {
        let mut goal = Vec::new();
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
                    number_len + marker.unwrap().len_utf8()
                };
                let title = trimmed[offset..].trim();
                if !title.is_empty() {
                    titles.push(title.to_owned());
                }
            } else if let Some(title) = titles.last_mut() {
                title.push('\n');
                title.push_str(line);
            } else {
                goal.push(line);
            }
        }
        (titles.len() > 1).then(|| {
            Self::new(
                if goal.join("\n").trim().is_empty() {
                    "Complete all requested tasks".into()
                } else {
                    goal.join("\n").trim().to_owned()
                },
                titles,
            )
        })
    }

    pub(crate) fn active(&self) -> bool {
        !self.summarized && self.stop_reason.is_none()
    }
    pub(crate) fn current_mut(&mut self) -> Option<&mut QueuedTask> {
        self.tasks
            .iter_mut()
            .find(|task| task.status == TaskStatus::Running)
    }
    pub(crate) fn advance(&mut self) {
        if !self
            .tasks
            .iter()
            .any(|task| task.status == TaskStatus::Running)
            && let Some(task) = self
                .tasks
                .iter_mut()
                .find(|task| task.status == TaskStatus::Pending)
        {
            task.status = TaskStatus::Running;
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
        if status == TaskStatus::Failed && task.recovery_attempts == 0 {
            task.failure_reason = Some(reason);
            return Err("attempt recovery first: search existing workspace runner/runtime/scripts and available environments, then retry or use an alternative".into());
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
                "completed_count": self.tasks.iter().filter(|task| task.status == TaskStatus::Completed).count(),
                "failed_count": self.tasks.iter().filter(|task| task.status == TaskStatus::Failed).count(),
                "remaining_tasks": self.tasks.iter().enumerate().filter(|(_,task)| task.status == TaskStatus::Pending).map(|(i,_)| i+1).collect::<Vec<_>>()
            })
        ))
    }
    pub(crate) fn snapshot(&self) -> Message {
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
        let queue = messages
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
                || (!m.content.starts_with(STATE_PREFIX) && !m.content.starts_with(PROGRESS_PREFIX))
        });
        queue
    }
    pub(crate) fn summary_context(&self) -> Message {
        Message::system(format!("[ax-task-summary]\nSummarize all task outcomes and failures; execution is complete. {}", json!(self.tasks.iter().enumerate().map(|(i,t)| json!({"task":i+1,"status":t.status,"outcome":t.outcome,"failure_reason":t.failure_reason})).collect::<Vec<_>>())))
    }
}

pub(crate) fn spec() -> ToolSpec {
    ToolSpec { kind: "function", function: FunctionSpec {
        name: TOOL_NAME.into(),
        description: "For requests containing multiple explicit subtasks, initialize the internal queue before executing any work unless a queue already exists. Use this tool alone in a round. Finish the current task with completed/failed/skipped and a concise outcome. Before declaring failure, search existing workspace runner/runtime/scripts and available environments and attempt recovery. Independent tasks continue after failure. Execute current_task only; a text-only response ends that task and the runtime advances the queue. Only summarize once current_task is null. For numbered requests the queue is automatic.".into(),
        parameters: json!({"type":"object","properties":{
            "action":{"type":"string","enum":["start","finish"]},
            "overall_goal":{"type":"string"},"tasks":{"type":"array","minItems":2,"items":{"type":"string"}},
            "status":{"type":"string","enum":["completed","failed","skipped"]},"reason":{"type":"string"}
        },"required":["action"]}),
    }}
}

pub(crate) fn apply(queue: &mut Option<TaskQueue>, input: &Value) -> Result<(), String> {
    match input["action"].as_str() {
        Some("start") => {
            if queue.as_ref().is_some_and(TaskQueue::active) {
                return Err("continue the existing queue; do not replan".into());
            }
            let goal = input["overall_goal"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or("overall_goal is required")?;
            let titles = input["tasks"]
                .as_array()
                .ok_or("tasks are required")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(str::to_owned)
                        .ok_or("task must be nonempty")
                })
                .collect::<Result<Vec<_>, _>>()?;
            if titles.len() < 2 {
                return Err("at least two tasks required".into());
            }
            *queue = Some(TaskQueue::new(goal.into(), titles));
            Ok(())
        }
        Some("finish") => {
            let status =
                serde_json::from_value(input["status"].clone()).map_err(|_| "invalid status")?;
            queue
                .as_mut()
                .filter(|q| q.active())
                .ok_or("no active queue")?
                .finish(status, input["reason"].as_str().unwrap_or("").into())
        }
        _ => Err("unknown task_queue action".into()),
    }
}

pub(crate) fn schema_tokens() -> usize {
    let spec = spec();
    crate::estimate_text_tokens(&spec.function.name)
        + crate::estimate_text_tokens(&spec.function.description)
        + crate::estimate_text_tokens(&spec.function.parameters.to_string())
}
