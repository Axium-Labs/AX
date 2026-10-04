//! Explicit opt-in stop verification. No guard is installed by default.
use crate::{AgentKernel, TurnState};
use async_trait::async_trait;
use model::{FunctionSpec, Message, ModelRequest, ToolSpec};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    #[default]
    Off,
    Deterministic,
    Model,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerificationConfig {
    pub mode: Verification,
    pub deliverables: Vec<PathBuf>,
    /// Explicit tool call IDs whose structured results must report success.
    pub required_successful_calls: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopDecision {
    Allow,
    Continue { reason: String },
}

#[async_trait]
pub trait StopGuard: Send + Sync {
    fn name(&self) -> &'static str {
        "custom"
    }
    fn uses_model(&self) -> bool {
        false
    }
    async fn evaluate(&self, state: &TurnState) -> StopDecision;
}

pub struct DeterministicStopGuard {
    pub config: VerificationConfig,
}
#[async_trait]
impl StopGuard for DeterministicStopGuard {
    fn name(&self) -> &'static str {
        "deterministic"
    }
    async fn evaluate(&self, state: &TurnState) -> StopDecision {
        if crate::needs_follow_up(state) {
            return StopDecision::Continue {
                reason: format!("Pending runtime work: {:?}", state.continuation()),
            };
        }
        for path in &self.config.deliverables {
            if !path.exists() {
                return StopDecision::Continue {
                    reason: format!("Required deliverable missing: {}", path.display()),
                };
            }
        }
        for id in &self.config.required_successful_calls {
            if !state.successful_tool_calls.contains(id) {
                return StopDecision::Continue {
                    reason: format!("Required successful tool result missing: {id}"),
                };
            }
        }
        StopDecision::Allow
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDecision {
    allow: bool,
    reason: String,
}

/// A separate, explicitly enabled model verifier. It cannot execute tools.
pub struct ModelStopGuard {
    pub provider: Arc<dyn model::ModelProvider>,
    pub retry_policy: model::RetryPolicy,
    pub deterministic: DeterministicStopGuard,
}

#[async_trait]
impl StopGuard for ModelStopGuard {
    fn uses_model(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "model"
    }
    async fn evaluate(&self, state: &TurnState) -> StopDecision {
        let decision = self.deterministic.evaluate(state).await;
        if decision != StopDecision::Allow {
            return decision;
        }
        let mut verifier = AgentKernel::new(
            self.provider.clone(),
            tool::ToolRegistry::default(),
            Arc::new(crate::AllowAll),
        );
        verifier.configure_retry(self.retry_policy);
        let mut history = state.evidence.clone();
        history.push(Message::system("[ax-stop-guard]\nExplicit optional verification: assess the requested deliverables against the recorded evidence. Return stop_decision with allow=true or allow=false and a concrete reason for continued execution. Do not execute tools."));
        let messages = match crate::context::request_context(&history, verifier.context_budget()) {
            Ok(messages) => messages,
            Err(error) => {
                return StopDecision::Continue {
                    reason: error.to_string(),
                };
            }
        };
        let request = ModelRequest {
            messages,
            tools: vec![ToolSpec {
                kind: "function",
                function: FunctionSpec {
                    name: "stop_decision".into(),
                    description: "Optional stop verification decision".into(),
                    parameters: serde_json::json!({"type":"object","properties":{"allow":{"type":"boolean"},"reason":{"type":"string"}},"required":["allow","reason"],"additionalProperties":false}),
                },
            }],
        };
        match verifier
            .request_with_retry_observed(
                request,
                |_| {},
                |_| {},
                || {
                    state
                        .guard_model_requests
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                },
            )
            .await
        {
            Ok(response) => {
                if response.tool_calls.len() == 1
                    && response.tool_calls[0].function.name == "stop_decision"
                    && let Ok(value) = serde_json::from_str::<ModelDecision>(
                        &response.tool_calls[0].function.arguments,
                    )
                {
                    if value.allow {
                        return StopDecision::Allow;
                    }
                    return StopDecision::Continue {
                        reason: value.reason,
                    };
                }
                StopDecision::Continue {
                    reason: "Stop guard returned no valid structured decision".into(),
                }
            }
            Err(error) => StopDecision::Continue {
                reason: format!("Stop guard failed: {error}"),
            },
        }
    }
}

impl AgentKernel {
    #[must_use]
    pub fn with_stop_guard(mut self, guard: Arc<dyn StopGuard>) -> Self {
        self.stop_guard = Some(guard);
        self
    }

    pub fn configure_verification(&mut self, config: VerificationConfig) {
        self.stop_guard = match config.mode {
            Verification::Off => None,
            Verification::Deterministic => Some(Arc::new(DeterministicStopGuard { config })),
            Verification::Model => Some(Arc::new(ModelStopGuard {
                provider: self.provider.clone(),
                retry_policy: self.retry_policy,
                deterministic: DeterministicStopGuard { config },
            })),
        };
    }
}
