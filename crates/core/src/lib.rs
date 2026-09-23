//! The provider-agnostic agent runtime kernel.

mod budget;
mod context;
pub use budget::{ContextBudget, ExecutionBudget};
pub use context::select_context;

use std::{collections::VecDeque, sync::Arc};

use async_trait::async_trait;
use model::{FunctionSpec, Message, ModelError, ModelProvider, ModelRequest, ToolSpec};
use serde_json::Value;
use thiserror::Error;
use tokio::task::JoinSet;
use tool::{SafetyLevel, ToolError, ToolOutput, ToolPermission, ToolRegistry};

#[derive(Clone, Debug)]
pub enum AgentEvent {
    TurnStarted,
    ModelStarted {
        provider: String,
        model: String,
    },
    ContentDelta {
        delta: String,
    },
    /// Streaming reasoning / chain-of-thought, shown ahead of the answer.
    ThinkingDelta {
        delta: String,
    },
    ToolStarted {
        name: String,
    },
    ToolFinished {
        name: String,
        success: bool,
    },
    TurnFinished,
    ContextCompressed {
        removed_messages: usize,
        estimated_tokens_before: usize,
        estimated_tokens_after: usize,
        tokens_freed: usize,
        compression_ratio: f64,
        cleanup_tier: &'static str,
        semantic_called: bool,
        tool_outputs_reduced: usize,
        tool_outputs_removed: usize,
        recent_raw_tokens: usize,
    },
}

#[derive(Clone, Debug)]
pub struct CompressionPolicy {
    pub threshold_percent: u8,
}

impl Default for CompressionPolicy {
    fn default() -> Self {
        Self {
            threshold_percent: ContextBudget::SOFT_PRESSURE_PERCENT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompressionResult {
    pub summary: String,
    pub removed_messages: usize,
    pub retained_messages: usize,
    pub estimated_tokens_before: usize,
    pub estimated_tokens_after: usize,
    pub tool_outputs_reduced: usize,
    pub tool_outputs_removed: usize,
    pub semantic_called: bool,
}

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("invalid tool arguments for {tool}: {source}")]
    InvalidToolArguments {
        tool: String,
        source: serde_json::Error,
    },
    #[error("agent exceeded the maximum of {0} model steps")]
    StepLimit(usize),
    #[error("execution budget exhausted: {0}")]
    Budget(String),
    #[error("history persistence failed: {0}")]
    Persistence(String),
    #[error("agent worker failed: {0}")]
    WorkerJoin(String),
}

#[async_trait]
pub trait ApprovalPolicy: Send + Sync {
    async fn approve(&self, tool: &str, input: &Value, permission: ToolPermission) -> bool;
}

pub struct DenyDangerous;

#[async_trait]
impl ApprovalPolicy for DenyDangerous {
    async fn approve(&self, _tool: &str, _input: &Value, permission: ToolPermission) -> bool {
        permission.safety == SafetyLevel::Safe
    }
}

pub struct AllowAll;

#[async_trait]
impl ApprovalPolicy for AllowAll {
    async fn approve(&self, _tool: &str, _input: &Value, _permission: ToolPermission) -> bool {
        true
    }
}

pub struct AgentKernel {
    provider: Arc<dyn ModelProvider>,
    tools: ToolRegistry,
    approval: Arc<dyn ApprovalPolicy>,
    messages: Vec<Message>,
    budget: ExecutionBudget,
    compression: CompressionPolicy,
    raw_turn_messages: Vec<Message>,
    compression_dirty: bool,
}

impl AgentKernel {
    #[must_use]
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        mut tools: ToolRegistry,
        approval: Arc<dyn ApprovalPolicy>,
    ) -> Self {
        if !provider.capabilities().vision {
            tools.remove("view_image");
        }
        Self {
            provider,
            tools,
            approval,
            messages: Vec::new(),
            budget: ExecutionBudget::default(),
            compression: CompressionPolicy::default(),
            raw_turn_messages: Vec::new(),
            compression_dirty: false,
        }
    }

    #[must_use]
    pub fn with_execution_budget(mut self, budget: ExecutionBudget) -> Self {
        self.budget = budget;
        self
    }

    #[must_use]
    pub fn with_tool(mut self, tool: impl tool::Tool + 'static) -> Self {
        self.tools.register(tool);
        self
    }

    /// Register or replace a tool before the next turn.
    pub fn register_tool(&mut self, tool: impl tool::Tool + 'static) {
        self.tools.register(tool);
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Seeds the kernel with a previously loaded session context.
    #[must_use]
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    #[must_use]
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.get(name).is_some()
    }

    /// Adds dynamically selected context, such as lazily loaded skill instructions.
    pub fn push_context(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Replace ephemeral retrieved context, rather than accumulating stale copies.
    pub fn set_context(&mut self, prefix: &str, message: Option<Message>) {
        self.messages
            .retain(|m| m.role != model::Role::System || !m.content.starts_with(prefix));
        if let Some(message) = message {
            self.messages.push(message);
        }
    }

    #[must_use]
    pub fn fork_with_messages(&self, messages: Vec<Message>) -> Self {
        Self {
            provider: Arc::clone(&self.provider),
            tools: self.tools.clone(),
            approval: Arc::clone(&self.approval),
            messages,
            budget: self.budget,
            compression: self.compression.clone(),
            raw_turn_messages: Vec::new(),
            compression_dirty: false,
        }
    }

    #[must_use]
    pub fn with_compression_policy(mut self, policy: CompressionPolicy) -> Self {
        self.compression = policy;
        self
    }

    #[must_use]
    pub fn estimated_context_tokens(&self) -> usize {
        estimate_tokens(&self.messages)
    }

    #[must_use]
    pub fn context_budget(&self) -> ContextBudget {
        ContextBudget::new(
            self.provider.context_window(),
            self.provider.max_output_tokens(),
            estimate_tool_schema_tokens(&self.tools),
        )
    }

    /// Raw messages produced by the last turn, independent of effective-context cleanup.
    pub fn take_turn_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.raw_turn_messages)
    }

    /// A snapshot is needed after compression so cleanup survives session resume.
    pub fn take_compression_dirty(&mut self) -> bool {
        std::mem::take(&mut self.compression_dirty)
    }

    /// Applies the layered compression pipeline when effective context is under
    /// pressure. Called before every model request in the agent loop.
    ///
    /// # Errors
    ///
    /// Returns an error when the summarization model call fails or returns no text.
    pub async fn compress_if_needed<F>(
        &mut self,
        emit: F,
    ) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compress(false, emit).await
    }

    /// Runs the same pipeline more aggressively on explicit user request.
    /// Recent messages and persistent system context remain protected.
    ///
    /// # Errors
    ///
    /// Returns an error when the summarization model call fails or produces
    /// an invalid response.
    pub async fn compact_now<F>(&mut self, emit: F) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compress(true, emit).await
    }

    #[allow(clippy::too_many_lines)]
    async fn compress<F>(
        &mut self,
        force: bool,
        mut emit: F,
    ) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let _timer = tool::telemetry::Timer::new("context.compress");
        let before = self.estimated_context_tokens();
        let original = self.messages.clone();
        let budget = self.context_budget();
        if !force && before < budget.compact_threshold(self.compression.threshold_percent) {
            return Ok(None);
        }
        let target = if force {
            budget.pressure_target() / 2
        } else {
            budget.pressure_target()
        };
        let need_to_free = before.saturating_sub(target);
        if need_to_free == 0 && !force {
            return Ok(None);
        }
        let recent_start = recent_raw_start(&self.messages, budget.recent_raw_budget());
        let recent_raw_tokens = estimate_tokens(&self.messages[recent_start..]);
        let mut reduced = 0;
        let mut tier = "cleanup";

        // Largest old, reproducible outputs first. Never mutate the raw turn log.
        let mut candidates = (0..recent_start)
            .filter(|&i| {
                self.messages[i].role == model::Role::Tool && self.messages[i].parts.is_empty()
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|&i| {
            std::cmp::Reverse(estimate_tokens(std::slice::from_ref(&self.messages[i])))
        });
        for i in candidates {
            if self.estimated_context_tokens() <= target && !force {
                break;
            }
            let old = self.messages[i].content.clone();
            if let Some(short) = compact_tool_output(&old) {
                self.messages[i].content = short;
                reduced += 1;
            }
        }

        // Repeated reads and repeated failures: keep the latest occurrence.
        if self.estimated_context_tokens() > target || force {
            tier = "deduplicate";
            let mut seen = std::collections::HashSet::new();
            let call_keys = self
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .map(|call| {
                    (
                        call.id.clone(),
                        format!("{}:{}", call.function.name, call.function.arguments),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>();
            for i in (0..self.messages.len()).rev() {
                if self.messages[i].role != model::Role::Tool || !self.messages[i].parts.is_empty()
                {
                    continue;
                }
                let content = self.messages[i].content.clone();
                let key = self.messages[i]
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| call_keys.get(id))
                    .cloned()
                    .unwrap_or(content.clone());
                if !seen.insert(key) && i < recent_start && estimate_text_tokens(&content) > 6 {
                    self.messages[i].content = "[duplicate tool output]".into();
                    reduced += 1;
                }
            }
        }

        let mut semantic_called = false;
        let mut removed_messages = 0;
        let mut tool_outputs_removed = 0;
        if self.estimated_context_tokens() > target || (force && reduced == 0) {
            // Only complete older turns are eligible. Existing summaries stay verbatim.
            let split = (recent_start..self.messages.len())
                .find(|&i| self.messages[i].role == model::Role::User)
                .unwrap_or(0);
            let old = &self.messages[..split];
            let transcript = old
                .iter()
                .filter(|m| m.role != model::Role::System)
                .map(|m| {
                    format!(
                        "{:?}: {}{}",
                        m.role,
                        m.content,
                        if m.tool_calls.is_empty() {
                            String::new()
                        } else {
                            format!("\nTool calls: {:?}", m.tool_calls)
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            if !transcript.is_empty() {
                tool_outputs_removed = old.iter().filter(|m| m.role == model::Role::Tool).count();
                let mut compression_messages = vec![Message::system(
                    "Compress the older conversation into the smallest state sufficient to continue the task correctly. Preserve information that may affect future decisions, such as important user constraints, decisions, unresolved problems, relevant failures, current progress, and necessary facts. Decide semantically what matters. Do not invent information or repeat information already preserved by an earlier summary. Return only JSON: {\"state\":[{\"type\":\"other\",\"content\":\"...\",\"importance\":0.8}]}. Choose each entry's type as appropriate, for example constraint, decision, goal, fact, progress, failure, error, next_action, or other. Include only entries that matter; no type is required. Importance must be between 0 and 1.",
                )];
                if let Some(previous) = old
                    .iter()
                    .find(|m| m.content.starts_with("[memory-summary]"))
                {
                    compression_messages.push(Message::system(format!("Already preserved session state, for reference only. Do not rewrite it:\n{}", previous.content)));
                }
                compression_messages.push(Message::user(transcript));
                let response = self
                    .provider
                    .complete(ModelRequest {
                        messages: compression_messages,
                        tools: Vec::new(),
                    })
                    .await
                    .map_err(|error| {
                        self.messages.clone_from(&original);
                        AgentError::Model(error)
                    })?;
                if response.content.trim().is_empty() {
                    self.messages = original;
                    return Err(AgentError::Model(ModelError::InvalidResponse(
                        "context summarizer returned empty text".into(),
                    )));
                }
                let Some(new_state) = parse_semantic_state(&response.content) else {
                    self.messages = original;
                    return Ok(None);
                };
                semantic_called = true;
                tier = "semantic";
                let mut retained = self.messages.split_off(split);
                let mut persistent = self
                    .messages
                    .drain(..)
                    .filter(|m| m.role == model::Role::System)
                    .collect::<Vec<_>>();
                removed_messages = split.saturating_sub(persistent.len());
                let mut state = persistent
                    .iter()
                    .find(|m| m.content.starts_with("[memory-summary]"))
                    .map_or_else(Vec::new, |m| parse_saved_summary(&m.content));
                for entry in new_state {
                    if let Some(existing) = state
                        .iter_mut()
                        .find(|saved| saved.content.eq_ignore_ascii_case(&entry.content))
                    {
                        if entry.importance > existing.importance {
                            *existing = entry;
                        }
                    } else {
                        state.push(entry);
                    }
                }
                let Some(summary) = fit_summary(&state, budget.session_summary_budget()) else {
                    self.messages = original;
                    return Ok(None);
                };
                persistent.retain(|m| !m.content.starts_with("[memory-summary]"));
                persistent.insert(0, Message::system(summary));
                persistent.append(&mut retained);
                self.messages = persistent;
            }
        }
        let after = self.estimated_context_tokens();
        if after >= before {
            self.messages = original;
            return Ok(None);
        }
        let summary = self
            .messages
            .iter()
            .find(|m| m.content.starts_with("[memory-summary]"))
            .map_or_else(String::new, |m| {
                m.content
                    .trim_start_matches("[memory-summary]\n")
                    .to_owned()
            });
        self.compression_dirty = true;
        let ratio_milli = u32::try_from(after.saturating_mul(1000) / before.max(1)).unwrap_or(1000);
        let compression_ratio = f64::from(ratio_milli) / 1000.0;
        eprintln!(
            "[context.compress] tokens_before={before} tokens_after={after} tokens_freed={} compression_ratio={:.3} cleanup_tier={tier} semantic_called={semantic_called} tool_outputs_reduced={reduced} tool_outputs_removed={tool_outputs_removed} recent_raw_tokens={recent_raw_tokens} need_to_free={need_to_free} hard_pressure={}",
            before - after,
            compression_ratio,
            before >= budget.hard_pressure_threshold()
        );
        emit(AgentEvent::ContextCompressed {
            removed_messages,
            estimated_tokens_before: before,
            estimated_tokens_after: after,
            tokens_freed: before - after,
            compression_ratio,
            cleanup_tier: tier,
            semantic_called,
            tool_outputs_reduced: reduced,
            tool_outputs_removed,
            recent_raw_tokens,
        });
        Ok(Some(CompressionResult {
            summary,
            removed_messages,
            retained_messages: self.messages.len(),
            estimated_tokens_before: before,
            estimated_tokens_after: after,
            tool_outputs_reduced: reduced,
            tool_outputs_removed,
            semantic_called,
        }))
    }

    /// Runs one user turn until the model returns a final response.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider or a tool fails structurally, tool
    /// arguments are invalid JSON, or a configured step limit is reached.
    ///
    /// # Panics
    ///
    /// Panics only if the event emitter's mutex is poisoned by a panic while
    /// a guard is held (unreachable in normal operation).
    pub async fn run_turn<F>(
        &mut self,
        input: impl Into<String>,
        emit: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.run_turn_checkpointed(input, emit, |_| Ok(())).await
    }

    /// Persist each complete message before advancing model or tool execution.
    ///
    /// # Errors
    /// Returns runtime or checkpoint failures. Already saved messages are not replayed.
    pub async fn run_turn_checkpointed<F, H>(
        &mut self,
        input: impl Into<String>,
        emit: F,
        mut checkpoint: H,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let result = if self.budget.turn_timeout_secs == 0 {
            self.run_turn_inner(input, emit, &mut checkpoint).await
        } else {
            tokio::time::timeout(
                std::time::Duration::from_secs(self.budget.turn_timeout_secs),
                self.run_turn_inner(input, emit, &mut checkpoint),
            )
            .await
            .unwrap_or_else(|_| Err(AgentError::Budget("turn timeout".into())))
        };
        if result.is_err() {
            // Persist a valid tool-call transcript even if a deadline interrupts execution.
            let answered = self
                .messages
                .iter()
                .filter_map(|m| m.tool_call_id.clone())
                .collect::<std::collections::HashSet<_>>();
            let pending = self
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .filter(|call| !answered.contains(&call.id))
                .map(|call| call.id.clone())
                .collect::<Vec<_>>();
            for id in pending {
                let interrupted = Message::tool(
                    id,
                    "Execution interrupted before a tool result was available.",
                );
                self.messages.push(interrupted.clone());
                self.raw_turn_messages.push(interrupted);
            }
        }
        checkpoint(&self.raw_turn_messages)?;
        result
    }

    #[allow(clippy::too_many_lines)]
    async fn run_turn_inner<F, H>(
        &mut self,
        input: impl Into<String>,
        emit: F,
        checkpoint: &mut H,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
        H: FnMut(&[Message]) -> Result<(), AgentError> + Send,
    {
        let emit = std::sync::Mutex::new(emit);
        (emit.lock().unwrap())(AgentEvent::TurnStarted);
        self.raw_turn_messages.clear();
        let user = Message::user(input);
        self.messages.push(user.clone());
        self.raw_turn_messages.push(user);
        checkpoint(&self.raw_turn_messages)?;
        let tool_specs = self
            .tools
            .iter()
            .map(|tool| ToolSpec {
                kind: "function",
                function: FunctionSpec {
                    name: tool.name().to_owned(),
                    description: tool.description().to_owned(),
                    parameters: tool.input_schema(),
                },
            })
            .collect::<Vec<_>>();

        let mut calls_used = 0;
        let mut steps_used = 0usize;
        loop {
            if self.budget.max_steps != 0 && steps_used >= self.budget.max_steps {
                return Err(AgentError::StepLimit(self.budget.max_steps));
            }
            steps_used = steps_used.saturating_add(1);
            self.compress_if_needed(|event| (emit.lock().unwrap())(event))
                .await?;
            (emit.lock().unwrap())(AgentEvent::ModelStarted {
                provider: self.provider.name().to_owned(),
                model: self.provider.model_id().to_owned(),
            });
            let mut on_delta = |delta: String| {
                (emit.lock().unwrap())(AgentEvent::ContentDelta { delta });
            };
            let mut on_thinking = |delta: String| {
                (emit.lock().unwrap())(AgentEvent::ThinkingDelta { delta });
            };
            let model_timer = tool::telemetry::Timer::new("model.request");
            let request_messages = request_context(&self.messages, self.context_budget())?;
            let response = self
                .provider
                .complete_stream(
                    ModelRequest {
                        messages: request_messages,
                        tools: tool_specs.clone(),
                    },
                    &mut on_delta,
                    &mut on_thinking,
                )
                .await?;
            drop(model_timer);
            let content = response.content;
            let tool_calls = response.tool_calls;
            let assistant = Message::assistant(content.clone(), tool_calls.clone());
            self.messages.push(assistant.clone());
            self.raw_turn_messages.push(assistant);
            checkpoint(&self.raw_turn_messages)?;

            if tool_calls.is_empty() {
                (emit.lock().unwrap())(AgentEvent::TurnFinished);
                return Ok(content);
            }

            if self.budget.max_tool_calls != 0
                && tool_calls.len() > self.budget.max_tool_calls.saturating_sub(calls_used)
            {
                return Err(AgentError::Budget("tool call limit".into()));
            }
            calls_used = calls_used.saturating_add(tool_calls.len());
            for call in tool_calls {
                let name = call.function.name;
                (emit.lock().unwrap())(AgentEvent::ToolStarted { name: name.clone() });
                let input: Value =
                    serde_json::from_str(&call.function.arguments).map_err(|source| {
                        AgentError::InvalidToolArguments {
                            tool: name.clone(),
                            source,
                        }
                    })?;
                let tool = self
                    .tools
                    .get(&name)
                    .ok_or_else(|| ToolError::Unknown(name.clone()))?;
                let approved = self
                    .approval
                    .approve(&name, &input, tool.permission(&input))
                    .await;
                let result = if approved {
                    let _timer = tool::telemetry::Timer::new(format!("tool.{}", tool.name()));
                    if self.budget.tool_timeout_secs == 0 {
                        tool.execute_output(input).await
                    } else {
                        tokio::time::timeout(
                            std::time::Duration::from_secs(self.budget.tool_timeout_secs),
                            tool.execute_output(input),
                        )
                        .await
                        .unwrap_or_else(|_| Err(ToolError::Execution("tool timeout".into())))
                    }
                } else {
                    Err(ToolError::PermissionDenied(name.clone()))
                };
                (emit.lock().unwrap())(AgentEvent::ToolFinished {
                    name,
                    success: result.is_ok(),
                });
                let tool_message =
                    match result.unwrap_or_else(|error| ToolOutput::Text(error.to_string())) {
                        ToolOutput::Text(text) => Message::tool(call.id, text),
                        ToolOutput::Image {
                            description,
                            media_type,
                            data,
                        } => {
                            let mut message = Message::tool(call.id, description.clone());
                            message.parts = vec![
                                model::ContentPart::Text { text: description },
                                model::ContentPart::Image { media_type, data },
                            ];
                            message
                        }
                    };
                self.messages.push(tool_message.clone());
                self.raw_turn_messages.push(tool_message);
                checkpoint(&self.raw_turn_messages)?;
            }
        }
    }
}

fn recent_raw_start(messages: &[Message], budget: usize) -> usize {
    let mut start = messages.len();
    let mut used: usize = 0;
    while start > 0 {
        let cost = estimate_tokens(&messages[start - 1..start]);
        if used.saturating_add(cost) > budget {
            break;
        }
        used += cost;
        start -= 1;
    }
    start
}

fn compact_tool_output(output: &str) -> Option<String> {
    if estimate_text_tokens(output) < 256 {
        return None;
    }
    let lines = output.lines().collect::<Vec<_>>();
    let mut kept: Vec<String> = Vec::new();
    for line in lines.iter().take(2) {
        kept.push(line.chars().take(240).collect());
    }
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if ([
            "error",
            "fail",
            "panic",
            "stack overflow",
            "passed",
            "exit=",
            "exit code",
            "warning:",
            "test result:",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || (line.contains(".rs:") && line.chars().any(|c| c.is_ascii_digit())))
            && !kept.iter().any(|saved| saved == line)
        {
            kept.push(line.chars().take(240).collect());
        }
        if kept.iter().map(String::len).sum::<usize>() > 1500 {
            break;
        }
    }
    let mut short = format!("[compressed tool output; {} original lines]\n", lines.len());
    for line in kept {
        short.push_str(&line);
        short.push('\n');
    }
    if estimate_text_tokens(&short) >= estimate_text_tokens(output) {
        None
    } else {
        Some(short)
    }
}

#[derive(Clone, Debug)]
struct StateEntry {
    kind: String,
    content: String,
    importance: f64,
}

fn parse_semantic_state(output: &str) -> Option<Vec<StateEntry>> {
    let value: Value = serde_json::from_str(output.trim()).ok()?;
    let entries = value.get("state")?.as_array()?;
    if entries.is_empty() {
        return None;
    }
    entries
        .iter()
        .map(|entry| {
            let kind = entry.get("type")?.as_str()?.trim();
            let content = entry.get("content")?.as_str()?.trim();
            let importance = entry.get("importance")?.as_f64()?;
            if kind.is_empty()
                || content.is_empty()
                || !importance.is_finite()
                || !(0.0..=1.0).contains(&importance)
            {
                return None;
            }
            Some(StateEntry {
                kind: kind.to_owned(),
                content: content.to_owned(),
                importance,
            })
        })
        .collect()
}

fn parse_saved_summary(summary: &str) -> Vec<StateEntry> {
    let content = summary
        .strip_prefix("[memory-summary]\n")
        .unwrap_or(summary);
    parse_semantic_state(content).unwrap_or_else(|| {
        vec![StateEntry {
            kind: "other".into(),
            content: content.to_owned(),
            importance: 0.8,
        }]
    })
}

fn serialize_state(entries: &[StateEntry]) -> String {
    let state = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "type": entry.kind, "content": entry.content, "importance": entry.importance,
            })
        })
        .collect::<Vec<_>>();
    format!("[memory-summary]\n{}", serde_json::json!({"state": state}))
}

fn fit_summary(entries: &[StateEntry], budget: usize) -> Option<String> {
    let mut ranked = (0..entries.len()).collect::<Vec<_>>();
    let score = |index: usize| {
        let entry = &entries[index];
        let recent = f64::from(u32::try_from(index).unwrap_or(u32::MAX))
            / f64::from(u32::try_from(entries.len().max(1)).unwrap_or(u32::MAX));
        let type_weight = match entry.kind.as_str() {
            "constraint" | "decision" | "goal" | "failure" | "error" => 0.015,
            "next_action" | "progress" => 0.008,
            _ => 0.0,
        };
        entry.importance + 0.03 * recent + type_weight
    };
    ranked.sort_by(|&a, &b| score(b).total_cmp(&score(a)).then_with(|| b.cmp(&a)));
    let mut selected = Vec::new();
    for index in ranked {
        let mut trial = selected.clone();
        trial.push(index);
        trial.sort_unstable();
        let candidate = serialize_state(
            &trial
                .iter()
                .map(|&i| entries[i].clone())
                .collect::<Vec<_>>(),
        );
        if estimate_tokens(&[Message::system(candidate)]) <= budget {
            selected = trial;
        }
    }
    if selected.is_empty() {
        return None;
    }
    Some(serialize_state(
        &selected
            .into_iter()
            .map(|i| entries[i].clone())
            .collect::<Vec<_>>(),
    ))
}

/// Selects request context without changing the effective in-memory transcript.
/// Old turns are removed first; optional retrieved context follows only when
/// it fits the same budget used to decide whether to compact.
fn request_context(
    messages: &[Message],
    budget: ContextBudget,
) -> Result<Vec<Message>, AgentError> {
    let mut history = Vec::new();
    let mut memory = Vec::new();
    let mut skills = Vec::new();
    for message in messages {
        if message.role == model::Role::System && message.content.starts_with("[retrieved-memory]")
        {
            memory.push(message.clone());
        } else if message.role == model::Role::System && message.content.starts_with("[ax-skill:") {
            skills.push(message.clone());
        } else {
            history.push(message.clone());
        }
    }

    let latest_turn_len = history
        .iter()
        .rposition(|message| message.role == model::Role::User)
        .map_or(0, |index| {
            history[index..]
                .iter()
                .filter(|message| message.role != model::Role::System)
                .count()
        });
    history = select_context(
        &history,
        budget.history_budget(),
        budget.session_summary_budget(),
    );
    if history
        .iter()
        .filter(|message| message.role != model::Role::System)
        .count()
        < latest_turn_len
    {
        return Err(AgentError::Budget(
            "latest user turn exceeds the input budget".into(),
        ));
    }
    let mut selected = history;
    let mut remaining = budget.usable().saturating_sub(estimate_tokens(&selected));
    // Routing and retrieval put their highest-priority entries first. When
    // space is tight, optional memory gives way before selected skills.
    for message in skills.into_iter().chain(memory) {
        let cost = estimate_tokens(std::slice::from_ref(&message));
        if cost <= remaining {
            selected.push(message);
            remaining -= cost;
        }
    }
    let estimated = estimate_tokens(&selected);
    if estimated > budget.usable() {
        return Err(AgentError::Budget(format!(
            "request needs {estimated} tokens, allowed {}",
            budget.usable()
        )));
    }
    Ok(selected)
}

/// Estimated token cost of the JSON tool schemas sent with every model
/// request, so context budgeting accounts for it instead of assuming
/// tool schemas are free.
#[must_use]
pub fn estimate_tool_schema_tokens(tools: &ToolRegistry) -> usize {
    tools
        .iter()
        .map(|tool| {
            estimate_text_tokens(tool.name())
                + estimate_text_tokens(tool.description())
                + estimate_text_tokens(&tool.input_schema().to_string())
        })
        .sum()
}

#[must_use]
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| {
            estimate_text_tokens(&message.content)
                + message
                    .parts
                    .iter()
                    .map(|part| match part {
                        model::ContentPart::Text { text } => estimate_text_tokens(text),
                        model::ContentPart::Image { .. } => 2048,
                    })
                    .sum::<usize>()
                + message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        estimate_text_tokens(&call.function.name)
                            + estimate_text_tokens(&call.function.arguments)
                    })
                    .sum::<usize>()
                + message
                    .tool_call_id
                    .as_ref()
                    .map_or(0, |id| estimate_text_tokens(id))
                + 4
        })
        .sum()
}

fn estimate_text_tokens(text: &str) -> usize {
    let (ascii, non_ascii) = text.chars().fold((0_usize, 0_usize), |counts, character| {
        if character.is_ascii() {
            (counts.0 + 1, counts.1)
        } else {
            (counts.0, counts.1 + 1)
        }
    });
    ascii.div_ceil(4) + non_ascii.saturating_mul(2)
}

#[derive(Clone, Debug)]
pub struct AgentTask {
    pub id: String,
    pub prompt: String,
    pub context: Vec<Message>,
}

#[derive(Debug)]
pub struct AgentTaskResult {
    pub id: String,
    pub result: Result<String, AgentError>,
}

#[derive(Clone, Debug)]
pub enum MultiAgentEvent {
    Started { id: String },
    Runtime { id: String, event: AgentEvent },
    Finished { id: String },
}

pub struct AgentSupervisor {
    template: AgentKernel,
    max_concurrency: usize,
}

impl AgentSupervisor {
    #[must_use]
    pub fn new(template: AgentKernel, max_concurrency: usize) -> Self {
        Self {
            template,
            max_concurrency: max_concurrency.max(1),
        }
    }

    /// Runs independent agent contexts with bounded concurrency.
    ///
    /// # Errors
    ///
    /// Individual model/tool failures are returned inside each task result.
    /// This method returns a top-level error only when a worker task panics or is cancelled.
    pub async fn run_tasks(
        &self,
        tasks: Vec<AgentTask>,
        events: Option<tokio::sync::mpsc::UnboundedSender<MultiAgentEvent>>,
    ) -> Result<Vec<AgentTaskResult>, AgentError> {
        let mut pending = tasks.into_iter().collect::<VecDeque<_>>();
        let mut running = JoinSet::new();
        let mut results = Vec::with_capacity(pending.len());

        while !pending.is_empty() || !running.is_empty() {
            while running.len() < self.max_concurrency
                && let Some(task) = pending.pop_front()
            {
                let mut kernel = self.template.fork_with_messages(task.context);
                let sender = events.clone();
                running.spawn(async move {
                    if let Some(sender) = &sender {
                        let _ = sender.send(MultiAgentEvent::Started {
                            id: task.id.clone(),
                        });
                    }
                    let id = task.id;
                    let event_id = id.clone();
                    let result = kernel
                        .run_turn(task.prompt, |event| {
                            if let Some(sender) = &sender {
                                let _ = sender.send(MultiAgentEvent::Runtime {
                                    id: event_id.clone(),
                                    event,
                                });
                            }
                        })
                        .await;
                    if let Some(sender) = &sender {
                        let _ = sender.send(MultiAgentEvent::Finished { id: id.clone() });
                    }
                    AgentTaskResult { id, result }
                });
            }

            if let Some(result) = running.join_next().await {
                results.push(result.map_err(|error| AgentError::WorkerJoin(error.to_string()))?);
            }
        }
        results.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use model::{FunctionCall, ModelResponse, ToolCall};
    use serde_json::json;

    use super::*;
    use tool::Tool;

    struct ScriptedProvider {
        model: String,
        responses: Mutex<VecDeque<ModelResponse>>,
    }

    #[async_trait]
    impl ModelProvider for ScriptedProvider {
        fn name(&self) -> &'static str {
            "test"
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            &self.model
        }

        fn context_window(&self) -> usize {
            6_000
        }

        fn max_output_tokens(&self) -> Option<usize> {
            Some(100)
        }

        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
            self.responses
                .lock()
                .expect("response mutex poisoned")
                .pop_front()
                .ok_or_else(|| ModelError::InvalidResponse("script exhausted".to_owned()))
        }
    }

    struct EchoTool;

    #[tokio::test]
    async fn checkpoint_failure_stops_before_tool_execution_and_keeps_recoverable_history() {
        let provider = ScriptedProvider {
            model: "scripted".into(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "echo".into(),
                        arguments: "{}".into(),
                    },
                }],
                finish_reason: None,
            }])),
        };
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));
        let mut saved = Vec::new();
        let mut failed_once = false;
        let result = kernel
            .run_turn_checkpointed(
                "test",
                |_| {},
                |messages| {
                    if messages.len() == 2 && !failed_once {
                        failed_once = true;
                        return Err(AgentError::Persistence("disk failure".into()));
                    }
                    saved = messages.to_vec();
                    Ok(())
                },
            )
            .await;
        assert!(matches!(result, Err(AgentError::Persistence(_))));
        assert_eq!(saved.len(), 3);
        assert_eq!(saved[0].role, model::Role::User);
        assert!(saved[2].content.contains("Execution interrupted"));
    }

    #[tokio::test]
    async fn each_model_and_tool_message_is_checkpointed_in_order() {
        let provider = ScriptedProvider {
            model: "scripted".into(),
            responses: Mutex::new(VecDeque::from([
                ModelResponse {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "echo".into(),
                            arguments: "{}".into(),
                        },
                    }],
                    finish_reason: None,
                },
                ModelResponse {
                    content: "done".into(),
                    tool_calls: vec![],
                    finish_reason: None,
                },
            ])),
        };
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));
        let mut lengths = Vec::new();
        kernel
            .run_turn_checkpointed(
                "test",
                |_| {},
                |messages| {
                    lengths.push(messages.len());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert_eq!(lengths, vec![1, 2, 3, 4, 4]);
    }

    struct EchoProvider;

    #[async_trait]
    impl ModelProvider for EchoProvider {
        fn name(&self) -> &'static str {
            "echo"
        }

        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            "echo-model"
        }

        fn context_window(&self) -> usize {
            6_000
        }

        fn max_output_tokens(&self) -> Option<usize> {
            Some(100)
        }

        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse {
                content: request
                    .messages
                    .last()
                    .map_or_else(String::new, |message| message.content.clone()),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            })
        }
    }

    #[async_trait]
    #[allow(clippy::unnecessary_literal_bound)]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }

        fn description(&self) -> &str {
            "Echo input"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn capability(&self, _input: &Value) -> tool::Capability {
            tool::Capability::Process
        }
        fn safety(&self, _input: &Value) -> SafetyLevel {
            SafetyLevel::Safe
        }

        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            Ok(input.to_string())
        }
    }

    #[tokio::test]
    async fn loops_through_tool_result_to_final_answer() {
        let provider = ScriptedProvider {
            model: "scripted".to_owned(),
            responses: Mutex::new(VecDeque::from([
                ModelResponse {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "call-1".to_owned(),
                        kind: "function".to_owned(),
                        function: FunctionCall {
                            name: "echo".to_owned(),
                            arguments: r#"{"value":"hello"}"#.to_owned(),
                        },
                    }],
                    finish_reason: Some("tool_calls".to_owned()),
                },
                ModelResponse {
                    content: "done".to_owned(),
                    tool_calls: Vec::new(),
                    finish_reason: Some("stop".to_owned()),
                },
            ])),
        };
        let mut registry = ToolRegistry::new();
        registry.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), registry, Arc::new(DenyDangerous));

        let answer = kernel
            .run_turn("use echo", |_| {})
            .await
            .expect("agent turn should succeed");

        assert_eq!(answer, "done");
        assert_eq!(kernel.messages().len(), 4);
        assert_eq!(kernel.messages()[2].role, model::Role::Tool);
    }

    #[tokio::test]
    async fn compresses_old_context_without_truncating_recent_messages() {
        let provider = ScriptedProvider {
            model: "scripted".to_owned(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                content: r#"{"state":[{"type":"goal","content":"goal and decisions preserved","importance":0.9}]}"#.to_owned(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            }])),
        };
        let messages = (0..6)
            .map(|index| Message::user(format!("{index}:{}", "x".repeat(3500))))
            .collect();
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::new(),
            Arc::new(DenyDangerous),
        )
        .with_messages(messages)
        .with_compression_policy(CompressionPolicy {
            threshold_percent: 1,
        });

        let result = kernel
            .compress_if_needed(|_| {})
            .await
            .expect("compression should succeed")
            .expect("context should exceed threshold");

        assert_eq!(result.removed_messages, 5);
        assert_eq!(kernel.messages().len(), 2);
        assert!(kernel.messages()[0].content.contains("goal and decisions"));
        assert!(kernel.messages()[1].content.starts_with("5:"));
    }

    #[test]
    fn request_context_fits_history_memory_skills_and_large_tool_schema() {
        let budget = ContextBudget::new(6_000, Some(500), 1_200);
        let mut messages = (0..30)
            .map(|index| Message::user(format!("old {index}:{}", "x".repeat(400))))
            .collect::<Vec<_>>();
        messages.push(Message::system(format!(
            "[retrieved-memory]\n{}",
            "m".repeat(500)
        )));
        messages.push(Message::system(format!(
            "[ax-skill:first]\n{}",
            "s".repeat(2_000)
        )));
        messages.push(Message::system(format!(
            "[ax-skill:second]\n{}",
            "s".repeat(2_000)
        )));
        messages.push(Message::user("current request"));

        let selected = request_context(&messages, budget).expect("request should fit");
        assert!(estimate_tokens(&selected) <= budget.usable());
        assert!(
            selected
                .iter()
                .any(|message| message.content == "current request")
        );
        assert!(
            selected
                .iter()
                .filter(|message| message.content.starts_with("old "))
                .count()
                < 30
        );
        assert_eq!(
            messages.len(),
            34,
            "selection must not mutate source history"
        );
    }

    #[test]
    fn tiny_context_rejects_an_oversized_latest_turn() {
        let budget = ContextBudget::new(1_000, Some(100), 700);
        let messages = vec![Message::user("x".repeat(2_000))];
        assert!(matches!(
            request_context(&messages, budget),
            Err(AgentError::Budget(_))
        ));
    }

    #[tokio::test]
    async fn compacted_context_stays_within_final_request_budget() {
        let provider = ScriptedProvider {
            model: "scripted".to_owned(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                content:
                    r#"{"state":[{"type":"progress","content":"short summary","importance":0.8}]}"#
                        .to_owned(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            }])),
        };
        let messages = (0..30)
            .map(|index| Message::user(format!("{index}:{}", "x".repeat(600))))
            .collect();
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::new(),
            Arc::new(DenyDangerous),
        )
        .with_messages(messages);
        assert!(kernel.compress_if_needed(|_| {}).await.unwrap().is_some());
        kernel.push_context(Message::system(format!(
            "[retrieved-memory]\n{}",
            "m".repeat(500)
        )));
        kernel.push_context(Message::system(format!(
            "[ax-skill:test]\n{}",
            "s".repeat(2_000)
        )));
        kernel.push_context(Message::user("new question"));
        let selected = request_context(kernel.messages(), kernel.context_budget()).unwrap();
        assert!(estimate_tokens(&selected) <= kernel.context_budget().usable());
        assert!(
            selected
                .iter()
                .any(|message| message.content == "new question")
        );
    }

    struct RecordingProvider {
        replies: Mutex<VecDeque<ModelResponse>>,
        request_tokens: Mutex<Vec<usize>>,
    }

    #[async_trait]
    impl ModelProvider for RecordingProvider {
        fn name(&self) -> &'static str {
            "recording"
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn model_id(&self) -> &str {
            "recording"
        }
        fn context_window(&self) -> usize {
            6_000
        }
        fn max_output_tokens(&self) -> Option<usize> {
            Some(100)
        }
        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
            if request
                .messages
                .first()
                .is_some_and(|m| m.content.starts_with("Compress the older conversation"))
            {
                return Ok(ModelResponse {
                    content: r#"{"state":[{"type":"goal","content":"finish","importance":0.9}]}"#
                        .into(),
                    tool_calls: vec![],
                    finish_reason: None,
                });
            }
            self.request_tokens
                .lock()
                .unwrap()
                .push(estimate_tokens(&request.messages));
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ModelError::InvalidResponse("script exhausted".into()))
        }
    }

    struct LargeTestTool;
    #[async_trait]
    impl Tool for LargeTestTool {
        #[allow(clippy::unnecessary_literal_bound)]
        fn name(&self) -> &str {
            "test"
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn description(&self) -> &str {
            "Run tests"
        }
        fn input_schema(&self) -> Value {
            json!({"type":"object"})
        }
        fn capability(&self, _: &Value) -> tool::Capability {
            tool::Capability::Process
        }
        fn safety(&self, _: &Value) -> SafetyLevel {
            SafetyLevel::Safe
        }
        async fn execute(&self, input: Value) -> Result<String, ToolError> {
            if input["large"] == true {
                Ok(format!(
                    "cargo test\n{}\ntest result: FAILED. 243 passed; 1 failed\nparser::tests::nested\nsrc/parser.rs:281 stack overflow\nexit=101",
                    "progress line\n".repeat(2000)
                ))
            } else {
                Ok("quick check passed".into())
            }
        }
    }

    fn test_call(id: &str, large: bool) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "test".into(),
                arguments: format!("{{\"large\":{large}}}"),
            },
        }
    }

    #[tokio::test]
    async fn compresses_between_tool_calls_and_preserves_raw_turn() {
        let provider = Arc::new(RecordingProvider {
            replies: Mutex::new(VecDeque::from([
                ModelResponse {
                    content: String::new(),
                    tool_calls: vec![test_call("a", true)],
                    finish_reason: None,
                },
                ModelResponse {
                    content: String::new(),
                    tool_calls: vec![test_call("b", false)],
                    finish_reason: None,
                },
                ModelResponse {
                    content: "done".into(),
                    tool_calls: vec![],
                    finish_reason: None,
                },
            ])),
            request_tokens: Mutex::new(vec![]),
        });
        let mut registry = ToolRegistry::new();
        registry.register(LargeTestTool);
        let mut kernel = AgentKernel::new(provider.clone(), registry, Arc::new(DenyDangerous));
        let mut events = Vec::new();
        assert_eq!(
            kernel
                .run_turn("run tests", |e| events.push(e))
                .await
                .unwrap(),
            "done"
        );
        let sizes = provider.request_tokens.lock().unwrap().clone();
        assert_eq!(sizes.len(), 3);
        assert!(
            sizes[1] < 1000,
            "second request should see reduced test output: {sizes:?}"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ContextCompressed {
                tool_outputs_reduced: 1,
                semantic_called: false,
                ..
            }
        )));
        assert!(
            kernel
                .messages()
                .iter()
                .any(|m| m.content.contains("243 passed; 1 failed"))
        );
        let raw = kernel.take_turn_messages();
        assert!(raw.iter().any(|m| m.content.len() > 20_000));
    }

    #[tokio::test]
    async fn semantic_state_keeps_early_constraints_and_failures_without_summary_recursion() {
        let provider = ScriptedProvider { model: "scripted".into(), responses: Mutex::new(VecDeque::from([
            ModelResponse { content: r#"{"state":[{"type":"constraint","content":"Do not change the public API","importance":1.0},{"type":"failure","content":"Approach A failed because of a parser stack overflow","importance":0.95}]}"#.into(), tool_calls: vec![], finish_reason: None }
        ])) };
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::new(),
            Arc::new(DenyDangerous),
        )
        .with_messages(vec![
            Message::system("[memory-summary]\nDECISIONS: keep SQLite"),
            Message::user(format!(
                "Do not change the public API\n{}",
                "task details ".repeat(1500)
            )),
            Message::assistant(
                "Approach A failed because of a parser stack overflow",
                vec![],
            ),
            Message::user("continue"),
        ]);
        let result = kernel.compact_now(|_| {}).await.unwrap().unwrap();
        assert!(result.semantic_called);
        assert!(result.summary.contains("Do not change the public API"));
        assert!(
            result
                .summary
                .contains("Approach A failed because of a parser stack overflow")
        );
        assert!(result.summary.contains("DECISIONS: keep SQLite"));
        assert_eq!(kernel.messages().last().unwrap().content, "continue");
    }

    #[tokio::test]
    async fn repeated_file_reads_collapse_older_tool_result() {
        let provider = Arc::new(RecordingProvider {
            replies: Mutex::new(VecDeque::new()),
            request_tokens: Mutex::new(vec![]),
        });
        let read = |id: &str| ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: "{\"path\":\"src/lib.rs\"}".into(),
            },
        };
        let mut kernel = AgentKernel::new(provider, ToolRegistry::new(), Arc::new(DenyDangerous))
            .with_messages(vec![
                Message::user("inspect file"),
                Message::assistant("", vec![read("first")]),
                Message::tool("first", "old file body \n".repeat(500)),
                Message::assistant("", vec![read("second")]),
                Message::tool("second", "new file body \n".repeat(500)),
                Message::user("current task"),
            ]);
        kernel.compact_now(|_| {}).await.unwrap();
        assert!(
            kernel
                .messages()
                .iter()
                .any(|m| m.content.contains("duplicate tool output"))
        );
    }

    #[test]
    fn structured_summary_uses_importance_over_keywords_and_type() {
        let entries = vec![
            StateEntry {
                kind: "other".into(),
                content: "All deliverables use British English".into(),
                importance: 0.95,
            },
            StateEntry {
                kind: "error".into(),
                content: "The word error appeared in a harmless example".into(),
                importance: 0.05,
            },
        ];
        let high_only = serialize_state(&entries[..1]);
        let budget = estimate_tokens(&[Message::system(high_only)]) + 2;
        let fitted = fit_summary(&entries, budget).unwrap();
        let parsed = parse_saved_summary(&fitted);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].content, "All deliverables use British English");
        assert!(estimate_tokens(&[Message::system(fitted)]) <= budget);
    }

    #[tokio::test]
    async fn semantic_model_can_preserve_constraint_without_keyword() {
        let provider = ScriptedProvider { model: "scripted".into(), responses: Mutex::new(VecDeque::from([
            ModelResponse { content: r#"{"state":[{"type":"constraint","content":"All deliverables use British English","importance":0.98}]}"#.into(), tool_calls: vec![], finish_reason: None }
        ])) };
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::new(),
            Arc::new(DenyDangerous),
        )
        .with_messages(vec![
            Message::user(format!(
                "All deliverables use British English\n{}",
                "background ".repeat(2000)
            )),
            Message::assistant("noted", vec![]),
            Message::user("continue"),
        ]);
        let result = kernel.compact_now(|_| {}).await.unwrap().unwrap();
        assert!(
            result
                .summary
                .contains("All deliverables use British English")
        );
        let selected = request_context(kernel.messages(), kernel.context_budget()).unwrap();
        assert!(estimate_tokens(&selected) <= kernel.context_budget().usable());
    }

    #[tokio::test]
    async fn malformed_structured_output_keeps_original_context() {
        let provider = ScriptedProvider {
            model: "scripted".into(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                content: "{broken JSON".into(),
                tool_calls: vec![],
                finish_reason: None,
            }])),
        };
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::new(),
            Arc::new(DenyDangerous),
        )
        .with_messages(vec![
            Message::user("long context ".repeat(2000)),
            Message::assistant("old answer", vec![]),
            Message::user("continue"),
        ]);
        let original = serde_json::to_string(kernel.messages()).unwrap();
        assert!(kernel.compact_now(|_| {}).await.unwrap().is_none());
        assert_eq!(serde_json::to_string(kernel.messages()).unwrap(), original);
        assert!(!kernel.take_compression_dirty());
    }

    #[tokio::test]
    async fn supervisor_runs_independent_tasks() {
        let template = AgentKernel::new(
            Arc::new(EchoProvider),
            ToolRegistry::new(),
            Arc::new(DenyDangerous),
        );
        let supervisor = AgentSupervisor::new(template, 2);
        let results = supervisor
            .run_tasks(
                vec![
                    AgentTask {
                        id: "b".to_owned(),
                        prompt: "second".to_owned(),
                        context: Vec::new(),
                    },
                    AgentTask {
                        id: "a".to_owned(),
                        prompt: "first".to_owned(),
                        context: Vec::new(),
                    },
                ],
                None,
            )
            .await
            .expect("supervisor should finish");

        assert_eq!(results[0].id, "a");
        assert_eq!(
            results[0]
                .result
                .as_deref()
                .expect("first agent should succeed"),
            "first"
        );
        assert_eq!(
            results[1]
                .result
                .as_deref()
                .expect("second agent should succeed"),
            "second"
        );
    }
    #[tokio::test]
    async fn tool_budget_stops_before_execution_and_keeps_valid_transcript() {
        let provider = ScriptedProvider {
            model: "test".into(),
            responses: Mutex::new(VecDeque::from([ModelResponse {
                content: String::new(),
                tool_calls: vec![
                    ToolCall {
                        id: "pending-1".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "echo".into(),
                            arguments: "{}".into(),
                        },
                    },
                    ToolCall {
                        id: "pending-2".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "echo".into(),
                            arguments: "{}".into(),
                        },
                    },
                ],
                finish_reason: None,
            }])),
        };
        let mut tools = ToolRegistry::new();
        tools.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), tools, Arc::new(AllowAll))
            .with_execution_budget(ExecutionBudget {
                max_tool_calls: 1,
                ..ExecutionBudget::default()
            });
        assert!(matches!(
            kernel.run_turn("test", |_| {}).await,
            Err(AgentError::Budget(_))
        ));
        assert_eq!(
            kernel.messages().last().unwrap().tool_call_id.as_deref(),
            Some("pending-2")
        );
        assert!(
            kernel
                .messages()
                .last()
                .unwrap()
                .content
                .contains("interrupted")
        );
    }

    struct PendingProvider;
    #[async_trait]
    impl ModelProvider for PendingProvider {
        fn name(&self) -> &'static str {
            "pending"
        }
        fn model_id(&self) -> &'static str {
            "pending"
        }
        fn context_window(&self) -> usize {
            1000
        }
        async fn complete(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn turn_timeout_is_enforced_while_model_is_waiting() {
        let mut kernel = AgentKernel::new(
            Arc::new(PendingProvider),
            ToolRegistry::new(),
            Arc::new(AllowAll),
        )
        .with_execution_budget(ExecutionBudget {
            turn_timeout_secs: 1,
            ..ExecutionBudget::default()
        });
        assert!(matches!(
            kernel.run_turn("wait", |_| {}).await,
            Err(AgentError::Budget(_))
        ));
        assert_eq!(kernel.messages()[0].content, "wait");
    }
}
