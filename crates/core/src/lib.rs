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
use tool::{SafetyLevel, ToolError, ToolPermission, ToolRegistry};

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
    },
}

#[derive(Clone, Debug)]
pub struct CompressionPolicy {
    pub threshold_percent: u8,
    pub retain_recent_messages: usize,
}

impl Default for CompressionPolicy {
    fn default() -> Self {
        Self {
            threshold_percent: 75,
            retain_recent_messages: 12,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompressionResult {
    pub summary: String,
    pub removed_messages: usize,
    pub retained_messages: usize,
    pub estimated_tokens_before: usize,
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
}

impl AgentKernel {
    #[must_use]
    pub fn new(
        provider: Arc<dyn ModelProvider>,
        tools: ToolRegistry,
        approval: Arc<dyn ApprovalPolicy>,
    ) -> Self {
        Self {
            provider,
            tools,
            approval,
            messages: Vec::new(),
            budget: ExecutionBudget::default(),
            compression: CompressionPolicy::default(),
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

    /// Summarizes old context when usage exceeds the configured fraction of
    /// the active model's context window. Recent messages are retained verbatim.
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

    /// Forces a context compaction regardless of the configured threshold.
    /// Recent messages and persistent system context are still preserved.
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

    async fn compress<F>(
        &mut self,
        force: bool,
        mut emit: F,
    ) -> Result<Option<CompressionResult>, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let _timer = tool::telemetry::Timer::new("context.compress");
        let estimated_tokens_before = self.estimated_context_tokens();
        let budget = self.context_budget();
        let threshold = budget.compact_threshold(self.compression.threshold_percent);
        let retain = self.compression.retain_recent_messages;
        if (!force && estimated_tokens_before < threshold) || self.messages.len() <= retain + 1 {
            return Ok(None);
        }
        let proposed = self.messages.len().saturating_sub(retain);
        let split = (0..=proposed)
            .rev()
            .find(|&index| self.messages[index].role == model::Role::User)
            .unwrap_or(0);
        if split == 0 {
            return Ok(None);
        }
        let old_messages = &self.messages[..split];
        let persistent_context = old_messages
            .iter()
            .filter(|message| {
                message.role == model::Role::System
                    && !message.content.starts_with("[memory-summary]")
            })
            .cloned()
            .collect::<Vec<_>>();
        let transcript = old_messages
            .iter()
            .filter(|message| {
                message.role != model::Role::System
                    || message.content.starts_with("[memory-summary]")
            })
            .map(|message| {
                format!(
                    "{:?}: {}{}",
                    message.role,
                    message.content,
                    if message.tool_calls.is_empty() {
                        String::new()
                    } else {
                        format!("\nTool calls: {:?}", message.tool_calls)
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        if transcript.is_empty() {
            return Ok(None);
        }
        let response = self
            .provider
            .complete(ModelRequest {
                messages: vec![
                    Message::system(
                        "Summarize prior agent-runtime context compactly. Preserve user goals, completed work, key code changes, tool outcomes, constraints, unresolved tasks, and important decisions. Do not invent facts.",
                    ),
                    Message::user(transcript),
                ],
                tools: Vec::new(),
            })
            .await?;
        if response.content.trim().is_empty() {
            return Err(AgentError::Model(ModelError::InvalidResponse(
                "context summarizer returned empty text".to_owned(),
            )));
        }
        let summary = response.content.trim().to_owned();
        let removed_messages = old_messages.len().saturating_sub(persistent_context.len());
        let retained = self.messages.split_off(split);
        let retained_messages = retained.len();
        self.messages = Vec::with_capacity(retained.len() + persistent_context.len() + 1);
        self.messages
            .push(Message::system(format!("[memory-summary]\n{summary}")));
        self.messages.extend(persistent_context);
        self.messages.extend(retained);
        emit(AgentEvent::ContextCompressed {
            removed_messages,
            estimated_tokens_before,
        });
        Ok(Some(CompressionResult {
            summary,
            removed_messages,
            retained_messages,
            estimated_tokens_before,
        }))
    }

    /// Runs one user turn until the model returns a final response.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider or a tool fails structurally, tool
    /// arguments are invalid JSON, or the bounded loop reaches its step limit.
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
        let timeout = std::time::Duration::from_secs(self.budget.turn_timeout_secs.max(1));
        let result = tokio::time::timeout(timeout, self.run_turn_inner(input, emit))
            .await
            .unwrap_or_else(|_| Err(AgentError::Budget("turn timeout".into())));
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
                self.messages.push(Message::tool(
                    id,
                    "Execution interrupted before a tool result was available.",
                ));
            }
        }
        result
    }

    async fn run_turn_inner<F>(
        &mut self,
        input: impl Into<String>,
        emit: F,
    ) -> Result<String, AgentError>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let emit = std::sync::Mutex::new(emit);
        (emit.lock().unwrap())(AgentEvent::TurnStarted);
        self.messages.push(Message::user(input));
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
        for _ in 0..self.budget.max_steps {
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
            self.messages
                .push(Message::assistant(content.clone(), tool_calls.clone()));

            if tool_calls.is_empty() {
                (emit.lock().unwrap())(AgentEvent::TurnFinished);
                return Ok(content);
            }

            if calls_used + tool_calls.len() > self.budget.max_tool_calls {
                return Err(AgentError::Budget("tool call limit".into()));
            }
            calls_used += tool_calls.len();
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
                    tokio::time::timeout(
                        std::time::Duration::from_secs(self.budget.tool_timeout_secs.max(1)),
                        tool.execute(input),
                    )
                    .await
                    .unwrap_or_else(|_| Err(ToolError::Execution("tool timeout".into())))
                } else {
                    Err(ToolError::PermissionDenied(name.clone()))
                };
                (emit.lock().unwrap())(AgentEvent::ToolFinished {
                    name,
                    success: result.is_ok(),
                });
                self.messages.push(Message::tool(
                    call.id,
                    result.unwrap_or_else(|error| error.to_string()),
                ));
            }
        }

        Err(AgentError::StepLimit(self.budget.max_steps))
    }
}

/// Selects request context without changing the durable in-memory transcript.
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
                content: "goal and decisions preserved".to_owned(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
            }])),
        };
        let messages = (0..6)
            .map(|index| Message::user(format!("{index}:{}", "x".repeat(100))))
            .collect();
        let mut kernel = AgentKernel::new(
            Arc::new(provider),
            ToolRegistry::new(),
            Arc::new(DenyDangerous),
        )
        .with_messages(messages)
        .with_compression_policy(CompressionPolicy {
            threshold_percent: 1,
            retain_recent_messages: 2,
        });

        let result = kernel
            .compress_if_needed(|_| {})
            .await
            .expect("compression should succeed")
            .expect("context should exceed threshold");

        assert_eq!(result.removed_messages, 4);
        assert_eq!(kernel.messages().len(), 3);
        assert!(kernel.messages()[0].content.contains("goal and decisions"));
        assert!(kernel.messages()[2].content.starts_with("5:"));
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
                content: "short summary".to_owned(),
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
                tool_calls: vec![ToolCall {
                    id: "pending".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "echo".into(),
                        arguments: "{}".into(),
                    },
                }],
                finish_reason: None,
            }])),
        };
        let mut tools = ToolRegistry::new();
        tools.register(EchoTool);
        let mut kernel = AgentKernel::new(Arc::new(provider), tools, Arc::new(AllowAll))
            .with_execution_budget(ExecutionBudget {
                max_tool_calls: 0,
                ..ExecutionBudget::default()
            });
        assert!(matches!(
            kernel.run_turn("test", |_| {}).await,
            Err(AgentError::Budget(_))
        ));
        assert_eq!(
            kernel.messages().last().unwrap().tool_call_id.as_deref(),
            Some("pending")
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
