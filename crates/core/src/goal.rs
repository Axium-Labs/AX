//! Optional target completion. The existing queue owns identity and checkpoints.
use crate::{AgentError, AgentKernel, TurnState};
use async_trait::async_trait;
use model::{FunctionSpec, Message, ModelRequest, ToolSpec};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
};
mod validation;
pub use validation::{
    GoalValidator, PendingValidation, ValidationCombination, ValidationEvidence, ValidatorBinding,
    ValidatorKind, WorkObservation, WorkStatus,
};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalMode {
    #[default]
    Off,
    Deterministic,
    Model,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GoalConfig {
    /// Host preference to apply this template to ordinary new tasks.
    /// Explicit Goal admission and saved Goal lifecycle remain independent.
    pub enabled: bool,
    pub mode: GoalMode,
    pub long_project: Option<crate::long_project::LongProjectConfig>,
    pub session_rotation: Option<crate::long_project::RotationConfig>,
    pub acceptance: Vec<String>,
    pub deliverables: Vec<PathBuf>,
    pub required_successful_calls: Vec<String>,
    pub validators: Vec<GoalValidator>,
    pub validation_combination: ValidationCombination,
    /// Cumulative model steps, including verification; existing retries belong to one step.
    /// Zero leaves the existing execution budget unchanged.
    pub max_rounds: usize,
    pub max_seconds: u64,
    pub max_tokens: u64,
    /// Enforced only for complete reported billing; explicit rates are display estimates.
    pub max_cost_microusd: u64,
    pub input_microusd_per_million: Option<u64>,
    pub output_microusd_per_million: Option<u64>,
    pub max_stagnant_checks: usize,
    /// Require an idle interval as well as unchanged observations; tools are awaited inline.
    pub stagnant_seconds: u64,
    /// Finite fallback for unknown evidence, even when no wall-clock threshold is reached.
    pub max_unobserved_checks: usize,
    pub stagnation_action: StagnationAction,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagnationAction {
    #[default]
    Pause,
    RequestHuman,
}
impl Default for GoalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: GoalMode::Off,
            long_project: None,
            session_rotation: None,
            acceptance: vec![],
            deliverables: vec![],
            required_successful_calls: vec![],
            validators: vec![],
            validation_combination: ValidationCombination::All,
            max_rounds: 0,
            max_seconds: 0,
            max_tokens: 0,
            max_cost_microusd: 0,
            input_microusd_per_million: None,
            output_microusd_per_million: None,
            max_stagnant_checks: 3,
            stagnant_seconds: 300,
            max_unobserved_checks: 12,
            stagnation_action: StagnationAction::Pause,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GoalUsage {
    pub requests: usize,
    pub rounds: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated: bool,
    pub cost_microusd: Option<u64>,
    pub cost_estimated: bool,
    pub elapsed_ms: u64,
}
impl GoalUsage {
    #[must_use]
    pub fn cost_display(&self) -> String {
        self.cost_microusd.map_or_else(
            || "unavailable".into(),
            |cost| {
                format!(
                    "{cost} micro-USD{}",
                    if self.cost_estimated {
                        " (estimated)"
                    } else {
                        ""
                    }
                )
            },
        )
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalState {
    Created,
    Running,
    Verifying,
    Paused,
    Completed,
    Blocked,
    Failed,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GoalDecision {
    Allow,
    Continue { reason: String },
    InsufficientEvidence { reason: String },
    Blocked { reason: String },
    Failed { reason: String },
}
impl GoalDecision {
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Allow => "Goal achieved",
            Self::Continue { reason }
            | Self::InsufficientEvidence { reason }
            | Self::Blocked { reason }
            | Self::Failed { reason } => reason,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Goal {
    #[serde(default)]
    pub validator_bindings: Vec<ValidatorBinding>,
    #[serde(default)]
    pub validation_evidence: Vec<ValidationEvidence>,
    #[serde(default)]
    pub pending_validation: Option<PendingValidation>,
    #[serde(default)]
    pub evaluator_reference: Option<String>,
    #[serde(default)]
    pub last_progress_ms: u64,
    #[serde(default)]
    pub activity: WorkStatus,
    #[serde(default)]
    pub active_since_ms: Option<u64>,
    #[serde(default)]
    pub elapsed_base_ms: u64,
    #[serde(default)]
    pub successful_calls: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub accounted_children: std::collections::BTreeSet<String>,
    /// Conservative reservation saved before inference; charged on interrupted recovery.
    #[serde(default)]
    pub pending_usage: Option<GoalUsage>,
    pub target: String,
    pub config: GoalConfig,
    pub state: GoalState,
    pub usage: GoalUsage,
    pub checks: usize,
    pub stagnant_checks: usize,
    pub evidence_signature: Option<String>,
    pub last_decision: Option<GoalDecision>,
    pub stop_reason: Option<String>,
}
impl Goal {
    pub(crate) fn record_usage(
        &mut self,
        reported: Option<&serde_json::Value>,
        input_estimate: u64,
        output_estimate: u64,
        requests: usize,
    ) {
        let input = reported.and_then(|u| {
            u["input_tokens"]
                .as_u64()
                .or_else(|| u["prompt_tokens"].as_u64())
        });
        let output = reported.and_then(|u| {
            u["output_tokens"]
                .as_u64()
                .or_else(|| u["completion_tokens"].as_u64())
        });
        self.usage.estimated |= input.is_none() || output.is_none() || requests > 1;
        let retries = u64::try_from(requests.saturating_sub(1)).unwrap_or(u64::MAX);
        let input = input
            .unwrap_or(input_estimate)
            .saturating_add(input_estimate.saturating_mul(retries));
        let output = output
            .unwrap_or(output_estimate)
            .saturating_add(output_estimate.saturating_mul(retries));
        let actual_cost = reported.and_then(|u| u["cost_microusd"].as_u64());
        let estimated_cost = (self.config.input_microusd_per_million.is_some()
            && self.config.output_microusd_per_million.is_some())
        .then(|| request_cost(&self.config, input, output));
        self.usage.cost_estimated |= actual_cost.is_none() || requests > 1;
        let cost = actual_cost.or(estimated_cost);
        self.usage.cost_microusd = if self.usage.requests == 0 {
            cost
        } else {
            self.usage
                .cost_microusd
                .zip(cost)
                .map(|(a, b)| a.saturating_add(b))
        };
        self.usage.requests = self.usage.requests.saturating_add(requests);
        self.usage.input_tokens = self.usage.input_tokens.saturating_add(input);
        self.usage.output_tokens = self.usage.output_tokens.saturating_add(output);
    }
    pub(crate) fn new(target: String, config: GoalConfig) -> Self {
        Self {
            validator_bindings: vec![],
            validation_evidence: vec![],
            pending_validation: None,
            evaluator_reference: None,
            last_progress_ms: 0,
            activity: WorkStatus::Unknown,
            active_since_ms: None,
            elapsed_base_ms: 0,
            accounted_children: BTreeSet::default(),
            successful_calls: BTreeSet::default(),
            pending_usage: None,
            target,
            config,
            state: GoalState::Created,
            usage: GoalUsage::default(),
            checks: 0,
            stagnant_checks: 0,
            evidence_signature: None,
            last_decision: None,
            stop_reason: None,
        }
    }
}
#[async_trait]
pub trait GoalEvaluator: Send + Sync {
    fn policy_reference(&self) -> &'static str {
        self.name()
    }
    /// Configured checks run through the SAME scheduler, permissions and sandbox as tools.
    fn validators(&self) -> &[GoalValidator] {
        &[]
    }
    fn name(&self) -> &'static str {
        "custom"
    }
    fn uses_model(&self) -> bool {
        false
    }
    async fn evaluate(&self, state: &TurnState) -> GoalDecision;
}
pub struct DeterministicGoal {
    pub config: GoalConfig,
}
#[async_trait]
impl GoalEvaluator for DeterministicGoal {
    fn name(&self) -> &'static str {
        "deterministic"
    }
    async fn evaluate(&self, state: &TurnState) -> GoalDecision {
        if crate::needs_follow_up(state) {
            return GoalDecision::Continue {
                reason: format!("Pending runtime work: {:?}", state.continuation()),
            };
        }
        for path in &self.config.deliverables {
            if !path.exists() {
                return GoalDecision::Continue {
                    reason: format!("Required deliverable missing: {}", path.display()),
                };
            }
        }
        for id in &self.config.required_successful_calls {
            if !state.successful_tool_calls.contains(id) {
                return GoalDecision::Continue {
                    reason: format!("Required successful tool result missing: {id}"),
                };
            }
        }
        if self.config.deliverables.is_empty()
            && self.config.required_successful_calls.is_empty()
            && self.config.validators.is_empty()
        {
            return GoalDecision::InsufficientEvidence { reason: "Deterministic Goal needs explicit deliverables or successful call IDs; natural-language acceptance requires model mode.".into() };
        }
        if !self.config.acceptance.is_empty()
            && self.config.mode != GoalMode::Model
            && self.config.validators.is_empty()
        {
            return GoalDecision::InsufficientEvidence { reason: "Natural-language acceptance needs model mode; file existence and tool success alone cannot establish it.".into() };
        }
        GoalDecision::Allow
    }
}
pub struct ModelGoal {
    pub provider: Arc<dyn model::ModelProvider>,
    pub retry_policy: model::RetryPolicy,
    pub deterministic: DeterministicGoal,
}
#[async_trait]
impl GoalEvaluator for ModelGoal {
    fn name(&self) -> &'static str {
        "model"
    }
    fn uses_model(&self) -> bool {
        true
    }
    async fn evaluate(&self, state: &TurnState) -> GoalDecision {
        if !self.deterministic.config.deliverables.is_empty()
            || !self
                .deterministic
                .config
                .required_successful_calls
                .is_empty()
        {
            let decision = self.deterministic.evaluate(state).await;
            if decision != GoalDecision::Allow {
                return decision;
            }
        }
        let mut verifier = AgentKernel::new(
            self.provider.clone(),
            tool::ToolRegistry::default(),
            Arc::new(crate::AllowAll),
        );
        verifier.configure_retry(self.retry_policy);
        let mut history = state.evidence.clone();
        history.push(Message::system("[ax-goal-verifier]\nAssess ONLY the current Goal target and acceptance against execution evidence. Assistant claims are not proof. Return goal_decision with status allow, continue, blocked, failed or insufficient_evidence, and a concrete reason. Blocked requires external action/permission; failed is irrecoverable. Never execute tools or broaden permissions. File existence proves neither freshness nor correctness."));
        let messages = match crate::context::request_context(&history, verifier.context_budget()) {
            Ok(messages) => messages,
            Err(error) => {
                return GoalDecision::InsufficientEvidence {
                    reason: error.to_string(),
                };
            }
        };
        let request = ModelRequest {
            messages,
            tools: vec![ToolSpec {
                kind: "function",
                function: FunctionSpec {
                    name: "goal_decision".into(),
                    description: "Current Goal completion decision".into(),
                    parameters: serde_json::json!({"type":"object","properties":{"status":{"type":"string","enum":["allow","continue","blocked","failed","insufficient_evidence"]},"reason":{"type":"string"}},"required":["status","reason"],"additionalProperties":false}),
                },
            }],
        };
        let input_estimate = crate::token::estimate_tokens(&request.messages)
            + crate::token::estimate_text_tokens(
                &serde_json::to_string(&request.tools).unwrap_or_default(),
            );
        match verifier
            .request_with_retry_observed(
                request,
                |_| {},
                |_| {},
                || {
                    state
                        .goal_model_requests
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                },
            )
            .await
        {
            Ok(response) => {
                let mut usage = Message::system("[ax-goal-usage]");
                usage.usage = Some(
                    serde_json::json!({"provider":self.provider.name(),"model":self.provider.model_id(),"reported":response.usage.clone(),"estimated_input_tokens":input_estimate,"estimated_output_tokens":crate::token::estimate_text_tokens(&response.content) + crate::token::estimate_text_tokens(&serde_json::to_string(&response.tool_calls).unwrap_or_default())}),
                );
                if let Some(messages) = &state.goal_usage_messages {
                    messages.lock().unwrap().push(usage);
                }
                model_decision(&response)
            }
            Err(AgentError::Model(error))
                if error.error_class() == model::ErrorClass::Retryable =>
            {
                GoalDecision::Blocked {
                    reason: format!("Goal verifier exhausted existing provider retries: {error}"),
                }
            }
            Err(AgentError::Model(
                error @ (model::ModelError::Configuration(_)
                | model::ModelError::HttpStatus {
                    status: 401 | 403, ..
                }
                | model::ModelError::HttpResponse {
                    status: 401 | 403, ..
                }),
            )) => GoalDecision::Blocked {
                reason: format!("Goal verifier needs configuration or authorization: {error}"),
            },
            Err(error) => GoalDecision::Failed {
                reason: format!("Goal verifier failed: {error}"),
            },
        }
    }
}
fn model_decision(response: &model::ModelResponse) -> GoalDecision {
    if response.tool_calls.len() == 1
        && response.tool_calls[0].function.name == "goal_decision"
        && let Ok(value) =
            serde_json::from_str::<serde_json::Value>(&response.tool_calls[0].function.arguments)
        && let (Some(status), Some(reason)) = (value["status"].as_str(), value["reason"].as_str())
        && value.as_object().is_some_and(|v| v.len() == 2)
        && !reason.trim().is_empty()
    {
        let reason = reason.to_owned();
        return match status {
            "allow" => GoalDecision::Allow,
            "continue" => GoalDecision::Continue { reason },
            "blocked" => GoalDecision::Blocked { reason },
            "failed" => GoalDecision::Failed { reason },
            _ => GoalDecision::InsufficientEvidence { reason },
        };
    }
    GoalDecision::InsufficientEvidence {
        reason: "Goal verifier returned no valid structured decision".into(),
    }
}

impl AgentKernel {
    #[must_use]
    pub fn with_goal_evaluator(mut self, evaluator: Arc<dyn GoalEvaluator>) -> Self {
        self.pending_goal_evaluator = Some(evaluator);
        self.pending_goal = Some(GoalConfig {
            mode: GoalMode::Deterministic,
            ..GoalConfig::default()
        });
        self
    }
    /// Applies only to the next goal. Controller criteria are never inherited by workers.
    pub fn configure_goal(&mut self, config: GoalConfig) {
        self.pending_goal_evaluator = None;
        self.pending_goal = (config.mode != GoalMode::Off).then_some(config);
    }
    #[must_use]
    pub fn goal(&self) -> Option<&Goal> {
        self.task_queue.as_ref().and_then(|q| q.goal.as_ref())
    }
    pub(crate) fn install_goal(&mut self, config: Option<GoalConfig>) {
        let configured = config.clone();
        self.goal_evaluator = config.and_then(|config| match config.mode {
            GoalMode::Off => None,
            GoalMode::Deterministic => {
                Some(Arc::new(DeterministicGoal { config }) as Arc<dyn GoalEvaluator>)
            }
            GoalMode::Model => Some(Arc::new(ModelGoal {
                provider: self.provider.clone(),
                retry_policy: self.retry_policy,
                deterministic: DeterministicGoal { config },
            }) as Arc<dyn GoalEvaluator>),
        });
        if self.goal_evaluator.is_some()
            && let Some(custom) = self.pending_goal_evaluator.take()
        {
            self.goal_evaluator = Some(custom);
            self.pending_goal = None;
        }
        self.wrap_goal_validators(configured);
    }
    pub(crate) fn wrap_goal_validators(&mut self, config: Option<GoalConfig>) {
        if let Some(config) = config.filter(|c| !c.validators.is_empty())
            && let Some(inner) = self.goal_evaluator.take()
        {
            self.goal_evaluator = Some(Arc::new(validation::ConfiguredGoal {
                inner,
                validators: config.validators,
                combination: config.validation_combination,
            }));
        }
    }
    pub(crate) fn sync_goal_usage(&mut self) {
        if let Some(queue) = &mut self.task_queue
            && let Some(goal) = &mut queue.goal
        {
            if self
                .subagent_manager
                .as_ref()
                .is_some_and(|manager| manager.has_delegated_work())
            {
                goal.usage.estimated = true;
                goal.usage.cost_estimated = true;
            }
            if let Some(since) = goal.active_since_ms {
                goal.usage.elapsed_ms = goal
                    .elapsed_base_ms
                    .saturating_add(now_ms().saturating_sub(since));
            }
            for (id, result) in &self.child_results {
                if !queue.tasks.iter().any(|task| {
                    task.child
                        .as_ref()
                        .is_some_and(|run| run.session_id == result.child_id)
                }) {
                    continue;
                }
                if goal.accounted_children.insert(id.clone()) {
                    let reported = serde_json::json!({"input_tokens":result.metrics.input_tokens,"output_tokens":result.metrics.output_tokens});
                    goal.record_usage(
                        Some(&reported),
                        result.metrics.prompt_tokens,
                        result.metrics.completion_tokens,
                        result.metrics.model_rounds,
                    );
                    // Child billing/retry usage is not independently complete.
                    goal.usage.estimated = true;
                }
            }
        }
    }
    pub(crate) fn reserve_goal_checkpoint(&mut self, input: usize) {
        self.sync_goal_usage();
        let output = u64::try_from(
            self.provider
                .max_output_tokens()
                .unwrap_or(model::DEFAULT_OUTPUT_RESERVE_TOKENS),
        )
        .unwrap_or(u64::MAX);
        if let Some(goal) = self.task_queue.as_mut().and_then(|q| q.goal.as_mut()) {
            let mut reservation = goal.clone();
            let attempts = self.retry_policy.max_attempts.max(1);
            reservation.record_usage(
                None,
                u64::try_from(input).unwrap_or(u64::MAX),
                output,
                attempts,
            );
            goal.pending_usage = Some(reservation.usage);
        }
    }
    pub(crate) fn goal_budget_check(&self) -> Result<(), AgentError> {
        let Some(goal) = self.goal() else {
            return Ok(());
        };
        let config = &goal.config;
        let usage = &goal.usage;
        let reason = if config.max_tokens > 0
            && usage.requests > 0
            && !usage.estimated
            && usage.input_tokens.saturating_add(usage.output_tokens) >= config.max_tokens
        {
            Some("Goal reported-token budget")
        } else if config.max_cost_microusd > 0
            && !usage.cost_estimated
            && usage
                .cost_microusd
                .is_some_and(|cost| cost >= config.max_cost_microusd)
        {
            Some("Goal reported-cost budget")
        } else {
            None
        };
        reason.map_or(Ok(()), |reason| Err(AgentError::Budget(reason.into())))
    }
    /// Reuse Harness's measured advances; changing assistant claims is not progress.
    // One classification updates the same persisted baseline and activity atomically.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn observe_goal_progress(&mut self) -> Result<(), AgentError> {
        if self.goal().is_none() {
            return Ok(());
        }
        let state = self.turn_state();
        self.sync_goal_usage();
        let execution = self.execution.lock().unwrap().clone();
        let Some(queue) = self.task_queue.as_mut() else {
            return Ok(());
        };
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        execution.progress.advances.hash(&mut hash);
        for task in &queue.tasks {
            // Concrete queue transitions and measured child receipts, not model prose.
            format!("{:?}", task.status).hash(&mut hash);
            task.child.as_ref().map(|c| &c.session_id).hash(&mut hash);
        }
        let Some(goal) = queue.goal.as_mut() else {
            return Ok(());
        };
        for path in &goal.config.deliverables {
            if let Ok(meta) = path.metadata() {
                meta.len().hash(&mut hash);
                meta.modified().ok().hash(&mut hash);
            }
        }
        let now = now_ms();
        let work: Vec<_> = goal
            .validation_evidence
            .iter()
            .filter_map(|receipt| {
                let work = receipt.work.as_ref()?;
                (now.saturating_sub(receipt.observed_at_ms)
                    < work
                        .valid_for_seconds
                        .min(goal.config.stagnant_seconds.max(30))
                        .saturating_mul(1000))
                .then_some(work)
            })
            .collect();
        for observation in &work {
            observation.reference.hash(&mut hash);
            observation.progress.hash(&mut hash);
        }
        let waiting = work
            .iter()
            .any(|w| matches!(w.status, WorkStatus::Running | WorkStatus::WaitingExternal));
        let blocked = work.iter().any(|w| w.status == WorkStatus::Blocked);
        if blocked {
            goal.activity = WorkStatus::Blocked;
            return Err(AgentError::GoalStopped {
                state: GoalState::Blocked,
                reason: "Observed external work requires intervention".into(),
            });
        }
        if waiting {
            goal.activity = if work.iter().any(|w| w.status == WorkStatus::WaitingExternal) {
                WorkStatus::WaitingExternal
            } else {
                WorkStatus::Running
            };
            // External hosts own wake-up/retry. Never spend inference polling an
            // unchanging external task or mistake its wait for a failed Goal.
            return Err(AgentError::GoalStopped { state:GoalState::Paused, reason:"Goal waiting for observed background/external work; resume when the host reports a change".into() });
        }
        if state.running_children > 0 || state.pending_approvals > 0 || state.retry_state {
            goal.activity = WorkStatus::Running;
            return Ok(());
        }
        let signature = format!("{:x}", hash.finish());
        goal.stagnant_checks = if goal.evidence_signature.as_ref() == Some(&signature) {
            goal.stagnant_checks.saturating_add(1)
        } else {
            goal.last_progress_ms = goal.usage.elapsed_ms;
            0
        };
        goal.evidence_signature = Some(signature);
        goal.activity = if goal.stagnant_checks == 0 {
            WorkStatus::Running
        } else if work.iter().any(|w| w.status == WorkStatus::NoProgress)
            || execution.progress.no_progress
        {
            WorkStatus::NoProgress
        } else {
            WorkStatus::Unknown
        };
        let idle = goal.usage.elapsed_ms.saturating_sub(goal.last_progress_ms);
        if goal.config.max_stagnant_checks > 0
            && goal.stagnant_checks >= goal.config.max_stagnant_checks
            && (idle >= goal.config.stagnant_seconds.saturating_mul(1000)
                || goal.stagnant_checks
                    >= goal
                        .config
                        .max_unobserved_checks
                        .max(goal.config.max_stagnant_checks))
        {
            let state = match goal.config.stagnation_action {
                StagnationAction::Pause => GoalState::Paused,
                StagnationAction::RequestHuman => GoalState::Blocked,
            };
            return Err(AgentError::GoalStopped {
                state,
                reason: format!(
                    "Goal progress is unconfirmed after {} observations and {}ms idle; inspect execution evidence before resuming",
                    goal.stagnant_checks, idle
                ),
            });
        }
        Ok(())
    }
}
pub(crate) fn request_cost(config: &GoalConfig, input: u64, output: u64) -> u64 {
    input
        .saturating_mul(config.input_microusd_per_million.unwrap_or(0))
        .div_ceil(1_000_000)
        .saturating_add(
            output
                .saturating_mul(config.output_microusd_per_million.unwrap_or(0))
                .div_ceil(1_000_000),
        )
}

pub(crate) fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}
