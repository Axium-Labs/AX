//! Configuration adapters at the existing `GoalEvaluator` boundary.
use super::{GoalDecision, GoalEvaluator};
use crate::{AgentError, AgentEvent, AgentKernel, TurnState, scheduler};
use async_trait::async_trait;
use model::{FunctionCall, Message, ToolCall};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCombination {
    #[default]
    All,
    Any,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalValidator {
    pub id: String,
    pub version: String,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// Explicit promise from the policy author, never inferred for command/host checks.
    #[serde(default)]
    pub replay_safe: bool,
    #[serde(flatten)]
    pub kind: ValidatorKind,
}
const fn default_timeout() -> u64 {
    30
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValidatorKind {
    Command {
        command: String,
        reference: PathBuf,
    },
    /// Read-only HTTP GET through `WebTool`, including its network policy and bounded body.
    Http {
        url: String,
    },
    /// A registered host/MCP capability, not an arbitrary unsandboxed callback.
    Host {
        tool: String,
        input: Value,
    },
}
impl GoalValidator {
    fn tool_name(&self) -> &str {
        match &self.kind {
            ValidatorKind::Command { .. } => "shell",
            ValidatorKind::Http { .. } => "web",
            ValidatorKind::Host { tool, .. } => tool,
        }
    }
    #[must_use]
    pub fn replay_safe(&self) -> bool {
        self.replay_safe || matches!(self.kind, ValidatorKind::Http { .. })
    }
    fn input(&self, request: &Value) -> Result<Value, String> {
        match &self.kind {
            ValidatorKind::Command { command, .. } => {
                Ok(json!({"command":command,"stdin":request.to_string()}))
            }
            ValidatorKind::Http { url } => {
                let mut url = url::Url::parse(url).map_err(|e| e.to_string())?;
                if !matches!(url.scheme(), "http" | "https")
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err(
                        "HTTP validator requires an HTTP(S) URL without embedded credentials"
                            .into(),
                    );
                }
                url.query_pairs_mut()
                    .append_pair("ax_goal", &request.to_string());
                Ok(json!({"operation":"fetch","urls":[url.to_string()]}))
            }
            ValidatorKind::Host { input, tool } => {
                let mut input = input.clone();
                let object = input
                    .as_object_mut()
                    .ok_or("host validator input must be an object")?;
                if object
                    .keys()
                    .any(|key| key.starts_with("_ax_") || key == "$tool_result" || key == "ax_goal")
                {
                    return Err("validator input cannot contain scheduler control fields".into());
                }
                // The existing lazy MCP gateway routes arguments to the remote tool.
                let arguments = if tool == "mcp" {
                    if object.get("action").and_then(Value::as_str) != Some("call") {
                        return Err("MCP validators require action=call".into());
                    }
                    object
                        .entry("arguments")
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .ok_or("MCP validator arguments must be an object")?
                } else {
                    object
                };
                if arguments
                    .keys()
                    .any(|key| key.starts_with("_ax_") || key == "$tool_result" || key == "ax_goal")
                {
                    return Err(
                        "validator arguments cannot contain reserved context/control fields".into(),
                    );
                }
                // Reserved context is provided only by this concrete Goal.
                arguments.insert("ax_goal".into(), request.clone());
                Ok(input)
            }
        }
    }
}

impl super::GoalConfig {
    /// Configuration validation does not execute tools or imply host availability.
    ///
    /// # Errors
    /// Returns a diagnostic for invalid identities, deadlines or invocation arguments.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(c) = &self.long_project {
            c.validate()?;
        }
        if let Some(c) = &self.session_rotation {
            c.validate()?;
        }
        let mut ids = std::collections::BTreeSet::new();
        for validator in &self.validators {
            if validator.id.trim().is_empty()
                || validator.version.trim().is_empty()
                || !(1..=86_400).contains(&validator.timeout_seconds)
                || !ids.insert(&validator.id)
            {
                return Err("Validators require unique nonempty IDs, versions and timeouts from 1 to 86400 seconds".into());
            }
            validator.input(&json!({}))?;
            match &validator.kind {
                ValidatorKind::Command { command, reference }
                    if command.trim().is_empty() || reference.as_os_str().is_empty() =>
                {
                    return Err("Command validators require a command and a reference file".into());
                }
                ValidatorKind::Host { tool, .. } if tool.trim().is_empty() => {
                    return Err("Host validators require a registered tool name".into());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidatorBinding {
    pub id: String,
    pub version: String,
    pub tool: String,
    pub schema: Value,
    pub environment: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingValidation {
    pub validator_id: String,
    pub call_id: String,
    pub replay_safe: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Running,
    WaitingExternal,
    NoProgress,
    Blocked,
    #[default]
    Unknown,
}
/// An observation, not a new background task registry. Hosts retain task ownership.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkObservation {
    pub status: WorkStatus,
    pub reference: String,
    pub progress: Option<String>,
    pub valid_for_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidationEvidence {
    pub validator_id: String,
    pub version: String,
    pub call_id: String,
    pub observed_at_ms: u64,
    pub decision: GoalDecision,
    pub evidence: Value,
    pub work: Option<WorkObservation>,
}

pub(super) struct ConfiguredGoal {
    pub inner: Arc<dyn GoalEvaluator>,
    pub validators: Vec<GoalValidator>,
    pub combination: ValidationCombination,
}
#[async_trait]
impl GoalEvaluator for ConfiguredGoal {
    fn policy_reference(&self) -> &'static str {
        self.inner.policy_reference()
    }
    fn name(&self) -> &'static str {
        "configured"
    }
    fn validators(&self) -> &[GoalValidator] {
        &self.validators
    }
    fn uses_model(&self) -> bool {
        self.inner.uses_model()
    }
    async fn evaluate(&self, state: &TurnState) -> GoalDecision {
        let decisions = &state.goal_validation;
        if decisions.len() != self.validators.len() {
            return GoalDecision::InsufficientEvidence {
                reason: "Configured validators have not all produced current evidence".into(),
            };
        }
        // Configuration/transport/permission blockers and irrecoverable failures cannot
        // be hidden by an OR branch. ANY only combines valid acceptance conditions.
        for evidence in decisions {
            if matches!(
                evidence.decision,
                GoalDecision::Blocked { .. } | GoalDecision::Failed { .. }
            ) {
                return evidence.decision.clone();
            }
        }
        let allowed = match self.combination {
            ValidationCombination::All => {
                decisions.iter().all(|e| e.decision == GoalDecision::Allow)
            }
            ValidationCombination::Any => {
                decisions.iter().any(|e| e.decision == GoalDecision::Allow)
            }
        };
        if !allowed {
            return decisions
                .iter()
                .find(|e| matches!(e.decision, GoalDecision::InsufficientEvidence { .. }))
                .or_else(|| decisions.iter().find(|e| e.decision != GoalDecision::Allow))
                .map_or_else(
                    || GoalDecision::InsufficientEvidence {
                        reason: "No acceptance evidence".into(),
                    },
                    |e| e.decision.clone(),
                );
        }
        self.inner.evaluate(state).await
    }
}

impl AgentKernel {
    /// Availability is checked BEFORE execution, again after resume, and at verification.
    pub(crate) fn prepare_goal_validators(&mut self) -> Result<(), AgentError> {
        let Some(goal) = self.goal() else {
            return Ok(());
        };
        let blocked = |reason| AgentError::GoalStopped {
            state: super::GoalState::Blocked,
            reason,
        };
        if let Some(reference) = &goal.evaluator_reference
            && self
                .goal_evaluator
                .as_ref()
                .is_none_or(|e| e.policy_reference() != reference)
        {
            return Err(blocked(format!(
                "Host GoalEvaluator {reference} must be explicitly reinstalled on this host before resume"
            )));
        }
        if let Some(pending) = &goal.pending_validation
            && !pending.replay_safe
        {
            return Err(blocked(format!(
                "Validator {} was interrupted; external effects are unknown. Inspect its call {} and acknowledge the effects before resuming.",
                pending.validator_id, pending.call_id
            )));
        }
        goal.config.validate().map_err(&blocked)?;
        let mut bindings = Vec::new();
        for validator in &goal.config.validators {
            if let ValidatorKind::Command { command, reference } = &validator.kind
                && (command.trim().is_empty() || !reference.is_file())
            {
                return Err(blocked(format!(
                    "Command validator {} reference unavailable: {}",
                    validator.id,
                    reference.display()
                )));
            }
            let tool = self.tools.get(validator.tool_name()).ok_or_else(|| {
                blocked(format!(
                    "Validator {} requires unavailable capability {}",
                    validator.id,
                    validator.tool_name()
                ))
            })?;
            bindings.push(ValidatorBinding {
                id: validator.id.clone(),
                version: validator.version.clone(),
                tool: tool.name().into(),
                schema: tool.input_schema(),
                environment: matches!(validator.kind, ValidatorKind::Command { .. })
                    .then(|| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)),
            });
        }
        if !goal.validator_bindings.is_empty() && goal.validator_bindings != bindings {
            return Err(blocked("Saved validator bindings differ from this host's capabilities/environment; explicitly repair the host or replace the Goal policy".into()));
        }
        if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
            goal.validator_bindings = bindings;
        }
        Ok(())
    }

    /// Execute configured `GoalEvaluator` checks using ordinary runtime tool admission.
    // Keep admission, effects markers and result checkpoints in one ordered transaction.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn evaluate_goal_validators<F, H>(
        &mut self,
        validators: &[GoalValidator],
        emit: &Mutex<F>,
        checkpoint: &mut H,
        calls_used: &mut usize,
    ) -> Result<Vec<ValidationEvidence>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        self.prepare_goal_validators()?;
        let mut evidence = Vec::new();
        for validator in validators {
            if self.budget.max_tool_calls > 0 && *calls_used >= self.budget.max_tool_calls {
                return Err(AgentError::Budget(
                    "tool call limit (Goal validator)".into(),
                ));
            }
            *calls_used = calls_used.saturating_add(1);
            let goal = self.goal().expect("optional Goal enabled");
            let request = json!({"protocol":"ax.goal.v1","goal_id":self.goal_id,"target":goal.target,"acceptance":goal.config.acceptance,"validator_id":validator.id,"version":validator.version});
            let call_id = format!(
                "{}-validation-{}-{}",
                self.goal_id.as_deref().unwrap_or_default(),
                goal.checks,
                validator.id
            );
            let input = validator
                .input(&request)
                .map_err(|reason| AgentError::GoalStopped {
                    state: super::GoalState::Blocked,
                    reason,
                })?;
            if self
                .tools
                .get(validator.tool_name())
                .is_some_and(|tool| tool.execution_usage_incomplete(&input))
            {
                let goal = self.task_queue.as_mut().unwrap().goal.as_mut().unwrap();
                goal.usage.estimated = true;
                goal.usage.cost_estimated = true;
            }
            let call = ToolCall {
                id: call_id.clone(),
                kind: "function".into(),
                function: FunctionCall {
                    name: validator.tool_name().into(),
                    arguments: input.to_string(),
                },
            };
            let mut message = Message::assistant("", vec![call.clone()]);
            message.provider_metadata = Some(json!({"axGoalValidator":validator.id}));
            self.messages.push(message.clone());
            self.raw_turn_messages.push(message);
            self.task_queue
                .as_mut()
                .unwrap()
                .goal
                .as_mut()
                .unwrap()
                .pending_validation = Some(PendingValidation {
                validator_id: validator.id.clone(),
                call_id: call_id.clone(),
                replay_safe: validator.replay_safe(),
            });
            self.checkpoint_queue(checkpoint)?;
            self.checkpoint_goal_host().await?;
            let timeout = if self.budget.tool_timeout_secs == 0 {
                validator.timeout_seconds
            } else {
                validator.timeout_seconds.min(self.budget.tool_timeout_secs)
            };
            let jobs = scheduler::prepare(&[call], &self.tools)?;
            let mut result = None;
            let execution = self.execution.clone();
            let tool = self.tools.get(validator.tool_name()).unwrap();
            // One deadline covers admission, permissions and execution. Racing a second
            // execution timer would lose the ambiguous-side-effect recovery marker.
            let scheduled = scheduler::run(
                jobs,
                self.approval.clone(),
                self.permission_profiles.clone(),
                1,
                0,
                emit,
                Some(self.execution.clone()),
                |message, _, input| {
                    if let Ok(envelope) = serde_json::from_str::<tool::ToolResult>(&message.content)
                    {
                        execution
                            .lock()
                            .unwrap()
                            .record(&call_id, tool.as_ref(), input, &envelope);
                    }
                    result = Some(message.clone());
                    self.messages.push(message.clone());
                    self.raw_turn_messages.push(message);
                    checkpoint(&self.raw_turn_messages)
                },
            );
            // Include approvals and resource-lock waits in the configured timeout.
            let deadline =
                tokio::time::timeout(std::time::Duration::from_secs(timeout), scheduled).await;
            if let Ok(result) = deadline {
                result?;
            } else {
                let interrupted = Message::tool(
                    call_id.clone(),
                    serde_json::to_string(&tool::ToolResult::new(
                        false,
                        "Validator deadline exceeded; external effects are unknown".into(),
                    ))
                    .unwrap(),
                );
                self.messages.push(interrupted.clone());
                self.raw_turn_messages.push(interrupted.clone());
                result = Some(interrupted);
                if !validator.replay_safe() {
                    let raw = &result.as_ref().unwrap().content;
                    let mut receipt = parse_result(validator, &request, &call_id, raw);
                    receipt.decision = GoalDecision::Blocked {
                        reason: format!(
                            "Validator {} deadline exceeded; external effects are unknown",
                            validator.id
                        ),
                    };
                    if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
                        goal.validation_evidence
                            .retain(|old| old.validator_id != validator.id);
                        goal.validation_evidence.push(receipt);
                    }
                    self.result_reader
                        .0
                        .write()
                        .unwrap()
                        .insert(call_id.clone(), raw.clone());
                    self.checkpoint_queue(checkpoint)?;
                    self.checkpoint_goal_host().await?;
                    return Err(AgentError::GoalStopped {
                        state: super::GoalState::Blocked,
                        reason: format!(
                            "Validator {} timed out; inspect and acknowledge external effects before retrying",
                            validator.id
                        ),
                    });
                }
            }
            let result = result.expect("one scheduled validator result");
            let receipt = parse_result(validator, &request, &call_id, &result.content);
            if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
                goal.pending_validation = None;
                goal.validation_evidence
                    .retain(|old| old.validator_id != validator.id);
                goal.validation_evidence.push(receipt.clone());
            }
            self.result_reader
                .0
                .write()
                .unwrap()
                .insert(call_id, result.content);
            evidence.push(receipt);
            self.checkpoint_queue(checkpoint)?;
            self.checkpoint_goal_host().await?;
        }
        Ok(evidence)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    protocol: String,
    goal_id: String,
    validator_id: String,
    version: String,
    decision: GoalDecision,
    evidence: Value,
    #[serde(default)]
    work: Option<WorkObservation>,
}
// Preserve the full receipt/identity/error admission sequence as one parser.
#[allow(clippy::too_many_lines)]
fn parse_result(
    validator: &GoalValidator,
    request: &Value,
    call: &str,
    raw: &str,
) -> ValidationEvidence {
    let mut receipt = ValidationEvidence {
        validator_id: validator.id.clone(),
        version: validator.version.clone(),
        call_id: call.into(),
        observed_at_ms: super::now_ms(),
        decision: GoalDecision::InsufficientEvidence {
            reason: format!(
                "Validator {} returned no valid current evidence",
                validator.id
            ),
        },
        evidence: Value::Null,
        work: None,
    };
    let Ok(result) = serde_json::from_str::<tool::ToolResult>(raw) else {
        return receipt;
    };
    if result.permission_denied {
        receipt.decision = GoalDecision::Blocked {
            reason: format!("Permission denied for validator {}", validator.id),
        };
        return receipt;
    }
    if let Some(reason) = result.global_blocker {
        receipt.decision = GoalDecision::Blocked { reason };
        return receipt;
    }
    if result.status != "success" {
        receipt.decision = GoalDecision::InsufficientEvidence {
            reason: format!(
                "Validator {} execution error: {}",
                validator.id, result.summary
            ),
        };
        return receipt;
    }
    if result.truncated {
        receipt.decision = GoalDecision::InsufficientEvidence {
            reason: format!("Validator {} result was truncated", validator.id),
        };
        return receipt;
    }
    let text = match &validator.kind {
        ValidatorKind::Command { .. } => result
            .raw_output
            .strip_prefix("exit_code: 0\nstdout:\n")
            .and_then(|s| {
                s.rsplit_once("\nstderr:\n")
                    .map(|(stdout, _)| stdout.to_owned())
            }),
        ValidatorKind::Http { .. } => serde_json::from_str::<Value>(&result.raw_output)
            .ok()
            .and_then(|v| {
                let page = &v["pages"][0];
                (v["succeeded"] == 1
                    && v["failed"] == 0
                    && page["truncated"] == false
                    && page["content_type"]
                        .as_str()
                        .is_some_and(|t| t.starts_with("application/json")))
                .then(|| page["content"].as_str().map(str::to_owned))
                .flatten()
            }),
        ValidatorKind::Host { .. } => {
            // Host tools may return the receipt directly or in the standard MCP envelope.
            serde_json::from_str::<Value>(&result.raw_output)
                .ok()
                .and_then(|value| {
                    if value.get("protocol").is_some() {
                        return Some(result.raw_output.clone());
                    }
                    if value["isError"] == true {
                        return None;
                    }
                    if let Some(structured) =
                        value.get("structuredContent").filter(|v| v.is_object())
                    {
                        return Some(structured.to_string());
                    }
                    let content = value["content"].as_array()?;
                    (content.len() == 1 && content[0]["type"] == "text")
                        .then(|| content[0]["text"].as_str().map(str::to_owned))
                        .flatten()
                })
        }
    };
    let response = text
        .as_deref()
        .and_then(|text| serde_json::from_str::<Response>(text).ok());
    if let Some(response) = response {
        if response.protocol != "ax.goal.v1"
            || response.goal_id != request["goal_id"]
            || response.validator_id != validator.id
            || response.version != validator.version
        {
            receipt.decision = GoalDecision::Blocked {
                reason: format!(
                    "Validator {} protocol, identity or version mismatch",
                    validator.id
                ),
            };
        } else if response.decision.reason().trim().is_empty() {
            receipt.decision = GoalDecision::InsufficientEvidence {
                reason: format!(
                    "Validator {} returned no concrete decision reason",
                    validator.id
                ),
            };
        } else if response.evidence.is_null()
            || response.evidence == json!({})
            || response.evidence == json!([])
            || response.evidence == json!("")
        {
            // An explicit blocker/failure can be surfaced without proof of success.
            if response.decision != GoalDecision::Allow {
                receipt.decision = response.decision;
            }
        } else {
            receipt.decision = response.decision;
            receipt.evidence = response.evidence;
            receipt.work = response
                .work
                .filter(|work| !work.reference.trim().is_empty() && work.valid_for_seconds > 0);
            if let Some(work) = &receipt.work {
                match work.status {
                    WorkStatus::Running | WorkStatus::WaitingExternal => {
                        receipt.decision = GoalDecision::Continue {
                            reason: format!("Waiting for observed work {}", work.reference),
                        }
                    }
                    WorkStatus::Blocked => {
                        receipt.decision = GoalDecision::Blocked {
                            reason: format!(
                                "Observed work {} requires intervention",
                                work.reference
                            ),
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    receipt
}
