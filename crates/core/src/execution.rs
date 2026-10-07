//! Goal-bound execution metadata and completion-event based progress tracking.
use model::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::VecDeque,
    hash::{Hash, Hasher},
    path::PathBuf,
};
use tool::{Resource, Tool, ToolError, ToolResult};

pub const STATE_PREFIX: &str = "[ax-execution-state]\n";
pub const CONTEXT_PREFIX: &str = "[ax-execution]\n";
const WINDOW: usize = 8;
const RECOVERY_LIMIT: usize = 3;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecutionState {
    pub overall_goal: String,
    pub goal_id: String,
    pub current_step: String,
    pub expected_output: String,
    pub allowed_scope: Vec<PathBuf>,
    pub recovery_for: Option<String>,
    #[serde(default)]
    pub recovery_call_id: Option<String>,
    pub recent_actions: VecDeque<ExecutionEvent>,
    pub progress: Progress,
    #[serde(default)]
    pub step_status: StepStatus,
    failed_action: Option<u64>,
    failed_tool: Option<String>,
    failed_paths: Vec<PathBuf>,
    recovery_attempts: usize,
    step_declared: bool,
    workspace_root: PathBuf,
    resume_scope: Option<Vec<PathBuf>>,
    pub total_tool_calls: usize,
    pub history_complete: bool,
    observed_evidence: VecDeque<u64>,
    observed_mutations: VecDeque<u64>,
    child_cursors: std::collections::BTreeMap<String, (usize, usize)>,
}
/// A completed call is an observation even when it fails or returns no matches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Running,
    Observed,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Progress {
    pub tool_calls: usize,
    pub advances: usize,
    pub no_progress: bool,
    pub strategy_changes: usize,
    last_intervention: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub id: String,
    pub tool: String,
    pub step: String,
    pub input: String,
    pub success: bool,
    pub advanced: bool,
    pub output: String,
    input_fingerprint: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    goal_id: String,
    step: String,
    expected_output: String,
    scope: Vec<PathBuf>,
}

pub struct NoProgressDetector;
impl NoProgressDetector {
    #[must_use]
    pub fn stalled(events: &VecDeque<ExecutionEvent>) -> bool {
        events.len() >= WINDOW
            && events
                .iter()
                .rev()
                .take(WINDOW)
                .all(|event| !event.advanced)
    }
}
fn fingerprint(name: &str, input: &Value) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hash);
    let mut input = input.clone();
    if let Some(object) = input.as_object_mut() {
        object.remove("_ax_execution");
        object.remove("_ax_observe");
    }
    input.to_string().hash(&mut hash);
    hash.finish()
}
fn normalized(path: &std::path::Path) -> PathBuf {
    match Resource::path(path) {
        Resource::Path(path) => path,
        _ => unreachable!(),
    }
}
impl ExecutionState {
    pub(crate) fn fresh() -> Self {
        Self {
            history_complete: true,
            ..Self::default()
        }
    }
    pub fn begin(&mut self, goal: &str, id: &str, cwd: &std::path::Path) {
        let actions = std::mem::take(&mut self.recent_actions);
        let total = self.total_tool_calls;
        let history_complete = self.history_complete;
        *self = Self {
            overall_goal: goal.into(),
            goal_id: id.into(),
            current_step: goal.into(),
            expected_output: "deliver the requested result".into(),
            allowed_scope: vec![normalized(cwd)],
            workspace_root: normalized(cwd),
            total_tool_calls: total,
            history_complete,
            recent_actions: actions,
            ..Self::default()
        };
    }
    /// Bind before execution, after result references have been resolved. No prose/keyword routing.
    /// # Errors
    /// Rejects invalid bindings and paths outside the declared resource scope.
    /// Progress and recovery never determine tool admission.
    pub fn prepare(&mut self, tool: &dyn Tool, input: Value) -> Result<Value, ToolError> {
        let mut next = self.clone();
        let input = next.prepare_inner(tool, input)?;
        *self = next;
        Ok(input)
    }
    fn bind_step(&mut self, binding: Value) -> Result<(), ToolError> {
        let invalid =
            |reason: &str| ToolError::InvalidInput(format!("execution invariant: {reason}"));
        let binding: Binding =
            serde_json::from_value(binding).map_err(|e| invalid(&e.to_string()))?;
        if binding.goal_id != self.goal_id
            || binding.step.trim().is_empty()
            || binding.expected_output.trim().is_empty()
            || binding.scope.is_empty()
        {
            return Err(invalid(
                "step must reference the active original goal and declare output/scope",
            ));
        }
        let scope: Vec<_> = binding
            .scope
            .iter()
            .map(|path| normalized(&self.workspace_root.join(path)))
            .collect();
        if scope
            .iter()
            .any(|path| !path.starts_with(&self.workspace_root))
        {
            return Err(invalid(
                "a step may narrow scope; new steps must stay within the workspace",
            ));
        }
        if binding.step != self.current_step {
            self.clear_recovery();
        }
        self.current_step = binding.step;
        self.expected_output = binding.expected_output;
        self.allowed_scope = scope;
        self.step_declared = true;
        Ok(())
    }
    fn prepare_inner(&mut self, tool: &dyn Tool, mut input: Value) -> Result<Value, ToolError> {
        if let Some(scope) = self.resume_scope.take() {
            self.allowed_scope = scope.iter().map(|path| normalized(path)).collect();
        }
        let invalid =
            |reason: &str| ToolError::InvalidInput(format!("execution invariant: {reason}"));
        if let Some(binding) = input
            .as_object_mut()
            .and_then(|object| object.remove("_ax_execution"))
        {
            self.bind_step(binding)?;
        }
        let resources = tool.resources(&input);
        for resource in &resources {
            if let Resource::Path(raw_path) = &resource.resource
                && !normalized(raw_path).starts_with(&self.workspace_root)
                && !tool.runtime_owned_resources()
            {
                return Err(invalid(&format!(
                    "tool path {} is outside the current step scope {:?}",
                    raw_path.display(),
                    self.allowed_scope
                )));
            }
        }
        self.step_status = StepStatus::Running;
        Ok(input)
    }
    #[allow(clippy::too_many_lines)] // Keep event accounting and recovery transition atomic.
    pub fn record(&mut self, id: &str, tool: &dyn Tool, input: &Value, result: &ToolResult) {
        self.step_status = StepStatus::Observed;
        let success = result.status == "success";
        // Read/list/search success alone is observation, not a goal state transition.
        // Unknown effects (shell/MCP) require explicit result evidence, never an assumed write.
        let mutation_key = fingerprint(tool.name(), input);
        let mutation = tool
            .resources(input)
            .iter()
            .any(|r| r.write && r.resource != Resource::All)
            && !self.observed_mutations.contains(&mutation_key);
        if success && mutation {
            self.observed_mutations.push_back(mutation_key);
        }
        while self.observed_mutations.len() > WINDOW * 2 {
            self.observed_mutations.pop_front();
        }
        let evidence_available = input
            .get("_ax_observe")
            .and_then(Value::as_str)
            .and_then(|pointer| {
                serde_json::from_str::<Value>(&result.raw_output)
                    .ok()
                    .and_then(|v| v.pointer(pointer).cloned())
            })
            .is_some_and(|value| value == Value::Bool(true));
        let evidence_key = fingerprint(
            &self.current_step,
            &serde_json::json!([tool.name(), input.get("_ax_observe"), result.raw_output]),
        );
        let evidence = evidence_available && !self.observed_evidence.contains(&evidence_key);
        if success && evidence {
            self.observed_evidence.push_back(evidence_key);
        }
        while self.observed_evidence.len() > WINDOW * 2 {
            self.observed_evidence.pop_front();
        }
        let paths: Vec<_> = tool
            .resources(input)
            .into_iter()
            .filter_map(|r| match r.resource {
                Resource::Path(path) => Some(path),
                _ => None,
            })
            .collect();
        let retry = self.failed_action == Some(fingerprint(tool.name(), input))
            || (self.failed_tool.as_deref() == Some(tool.name())
                && !paths.is_empty()
                && paths == self.failed_paths);
        let advanced = success && (mutation || evidence || (self.recovery_for.is_some() && retry));
        self.progress.tool_calls += 1;
        self.total_tool_calls += 1;
        if advanced {
            self.progress.advances += 1;
            self.progress.no_progress = false;
        }
        if !success {
            if self.failed_action.is_none() {
                self.recovery_for = Some(self.current_step.clone());
                self.recovery_call_id = Some(id.into());
                self.failed_action = Some(fingerprint(tool.name(), input));
                self.failed_tool = Some(tool.name().into());
                self.failed_paths = paths;
            }
            self.recovery_attempts += 1;
            if self.recovery_attempts.is_multiple_of(RECOVERY_LIMIT) {
                self.progress.strategy_changes += 1;
                self.progress.no_progress = true;
                self.progress.last_intervention = self.progress.tool_calls;
            }
        } else if self.recovery_for.is_some()
            && (retry || (self.failed_action.is_none() && advanced))
        {
            self.clear_recovery();
        } else if success && self.recovery_for.is_some() {
            self.recovery_attempts = 0;
        }
        self.append_event(ExecutionEvent {
            id: id.into(),
            tool: tool.name().into(),
            step: self.current_step.clone(),
            input: input.to_string().chars().take(240).collect(),
            success,
            advanced,
            output: result.raw_output.chars().take(240).collect(),
            input_fingerprint: fingerprint(tool.name(), input),
        });
    }
    fn clear_recovery(&mut self) {
        self.recovery_for = None;
        self.recovery_call_id = None;
        self.failed_action = None;
        self.failed_tool = None;
        self.failed_paths.clear();
        self.recovery_attempts = 0;
        if let Some(scope) = self.resume_scope.take() {
            self.allowed_scope = scope;
        }
    }
    fn append_event(&mut self, event: ExecutionEvent) {
        self.recent_actions.push_back(event);
        while self.recent_actions.len() > WINDOW * 2 {
            self.recent_actions.pop_front();
        }
        if self.progress.tool_calls >= self.progress.last_intervention + WINDOW
            && NoProgressDetector::stalled(&self.recent_actions)
        {
            self.progress.no_progress = true;
            if self.recovery_for.is_none() {
                self.recovery_for = Some(self.current_step.clone());
            }
            self.progress.strategy_changes += 1;
            self.progress.last_intervention = self.progress.tool_calls;
        }
    }
    /// Record runtime controls and unavailable-tool attempts without fabricating an execution.
    pub(crate) fn record_control(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        result: &ToolResult,
        advanced: bool,
    ) {
        self.step_status = StepStatus::Observed;
        let success = result.status == "success";
        self.progress.tool_calls += 1;
        self.total_tool_calls += 1;
        if advanced && success {
            self.progress.advances += 1;
            self.progress.no_progress = false;
        }
        self.append_event(ExecutionEvent {
            id: id.into(),
            tool: name.into(),
            step: self.current_step.clone(),
            input: input.to_string().chars().take(240).collect(),
            success,
            advanced: advanced && success,
            output: result.raw_output.chars().take(240).collect(),
            input_fingerprint: fingerprint(name, input),
        });
    }
    /// Child receipts and history are separate, but visible tool events belong in controller history too.
    pub fn absorb_child(&mut self, session: &str, child: &Self) -> bool {
        let cursor = self.child_cursors.entry(session.into()).or_default();
        let calls = child.total_tool_calls.saturating_sub(cursor.0);
        if calls == 0 {
            return false;
        }
        self.total_tool_calls += calls;
        self.progress.tool_calls += calls;
        self.progress.advances += child.progress.advances.saturating_sub(cursor.1);
        *cursor = (child.total_tool_calls, child.progress.advances);
        let mut events: Vec<_> = child
            .recent_actions
            .iter()
            .rev()
            .take(calls)
            .cloned()
            .collect();
        events.reverse();
        for mut event in events {
            event.id = format!("{session}:{}", event.id);
            self.recent_actions.push_back(event);
        }
        while self.recent_actions.len() > WINDOW * 2 {
            self.recent_actions.pop_front();
        }
        true
    }
    /// # Panics
    /// Panics if a configured scope path cannot be serialized as UTF-8.
    #[must_use]
    pub fn snapshot(&self) -> Message {
        Message::system(format!(
            "{STATE_PREFIX}{}",
            serde_json::to_string(self).unwrap()
        ))
    }
    #[must_use]
    pub fn context(&self, event_count: usize) -> Message {
        let events: Vec<_> = self.recent_actions.iter().rev().take(event_count).collect();
        Message::system(format!(
            "{CONTEXT_PREFIX}{}",
            serde_json::json!({
                "goal_id":self.goal_id,"overall_goal":self.overall_goal,"current_step":self.current_step,
                "expected_output":self.expected_output,"allowed_scope":self.allowed_scope,"recovery_for":self.recovery_for,
                "step_status":self.step_status,"recovery_call_id":self.recovery_call_id,"failed_tool":self.failed_tool,"failed_resources":self.failed_paths,"progress":self.progress,"total_tool_calls":self.history_complete.then_some(self.total_tool_calls),"known_tool_calls":self.total_tool_calls,"history_complete":self.history_complete,"actual_recent_tool_events":events,
                "next_action":if self.progress.no_progress {"No progress: consider retry, diagnosis or replanning using actual observations. Empty results and no-match are valid observations; tools remain available."} else if self.recovery_for.is_some() {"Consider retrying the failed operation, diagnosing with other tools, replanning or continuing an independent step. Recovery is advisory; permissions and resource boundaries still apply."} else {"Bind new steps with _ax_execution. Answer execution-history questions from actual events; omitted events are unknown, not zero calls."}
            })
        ))
    }
    /// Backfill only actual retained call/result pairs; compressed conversation is not evidence.
    pub(crate) fn seed_history(&mut self, messages: &[Message]) {
        self.history_complete = !messages
            .iter()
            .any(|m| m.role != model::Role::System || m.content.starts_with("[memory-summary]"));
        let calls: std::collections::HashMap<_, _> = messages
            .iter()
            .flat_map(|m| &m.tool_calls)
            .map(|call| (call.id.as_str(), call))
            .collect();
        for message in messages.iter().filter(|m| m.role == model::Role::Tool) {
            let Some(call) = message.tool_call_id.as_deref().and_then(|id| calls.get(id)) else {
                continue;
            };
            let result = serde_json::from_str::<ToolResult>(&message.content)
                .unwrap_or_else(|_| ToolResult::from_legacy(message.content.clone()));
            self.total_tool_calls += 1;
            self.recent_actions.push_back(ExecutionEvent {
                id: call.id.clone(),
                tool: call.function.name.clone(),
                step: "historical step unavailable".into(),
                input: call.function.arguments.chars().take(240).collect(),
                success: result.status == "success",
                advanced: false,
                output: result.raw_output.chars().take(240).collect(),
                input_fingerprint: fingerprint(
                    &call.function.name,
                    &serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null),
                ),
            });
            while self.recent_actions.len() > WINDOW * 2 {
                self.recent_actions.pop_front();
            }
        }
    }
    pub fn restore(messages: &mut Vec<Message>) -> Option<Self> {
        let state = messages
            .iter()
            .rev()
            .filter(|m| m.role == model::Role::System)
            .find_map(|m| {
                m.content
                    .strip_prefix(STATE_PREFIX)
                    .and_then(|json| serde_json::from_str(json).ok())
            });
        messages.retain(|m| {
            m.role != model::Role::System
                || (!m.content.starts_with(STATE_PREFIX) && !m.content.starts_with(CONTEXT_PREFIX))
        });
        state
    }
}
