//! Turn CLI configuration into runtime objects.
//!
//! The builder is the only place that knows how a `ModelSelection` becomes a
//! concrete provider, a tool registry and an `AgentKernel`. It stays lazy:
//! calling it is what constructs a provider, never process startup.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Result, anyhow};
use mcp::McpToolProxy;
use model::{
    AuthStorage, HedgeConfig, HedgingProvider, Message, ModelProvider, OpenAiCompatibleProvider,
    OpenAiConfig, OpenAiProvider,
};
use runtime_core::{AgentKernel, ApprovalPolicy};
use tool::{FilesystemTool, ShellTool, ToolRegistry};

use crate::{
    args::Cli,
    bootstrap::discover_project_root,
    config::{AxConfig, InferenceMode},
    model_selection::{self, ModelSelection, ProviderKind},
};

pub(crate) fn tools(mcp_tools: &[McpToolProxy]) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let root = tool_workspace();
    if std::env::var_os("AX_SSH_CONTEXT").is_some()
        || std::env::var_os("AX_SSH_CONTEXT_FILE").is_some()
    {
        // Remote SSH work must not accidentally mutate a matching local path.
        if let Ok(Some(ssh)) = tool::SshTool::from_env() {
            registry.register(ssh);
        }
        registry.register(tool::WebTool::new());
        return registry;
    }
    // Every local tool is bound to the same workspace root, so the main agent
    // and a forked child differ only by that binding, never by implementation.
    for local in [
        Arc::new(tool::TaskSourceTool::new(root.clone())) as Arc<dyn tool::Tool>,
        Arc::new(ShellTool) as Arc<dyn tool::Tool>,
        Arc::new(FilesystemTool),
        Arc::new(tool::PatchTool),
        Arc::new(tool::FindFilesTool::new(root.clone())),
        Arc::new(tool::FindFilesTool::glob(root.clone())),
        Arc::new(tool::SearchTool::new(root.clone())),
        Arc::new(tool::ViewImageTool::new(root.clone())),
    ] {
        registry.register(tool::SandboxedTool::new(local, root.clone()));
    }
    registry.register(tool::WebTool::new());
    if let Some(collaboration) = crate::distributed_tool::CollaborationTool::from_env(root) {
        registry.register(collaboration);
    }
    for tool in mcp_tools {
        registry.register(tool.clone());
    }
    registry
}
pub(crate) fn tool_workspace() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    discover_project_root(&cwd)
}
/// One run's provider, final tool registry, and the context budget measured
/// against exactly that registry.
///
/// Building is the expensive part: a provider performs auth/catalog work and a
/// registry constructs and binds every tool. Both happen once per run, and the
/// same registry instance is handed to the kernel, so the schema estimate can
/// never describe a different tool set than the one that is sent.
pub(crate) struct Runtime {
    provider: Arc<dyn ModelProvider>,
    tools: ToolRegistry,
    budget: runtime_core::ContextBudget,
    /// Controller auth override, needed to build configured child models.
    codex_auth: Option<PathBuf>,
}

impl Runtime {
    /// Build a run's provider and final tool registry exactly once each.
    pub(crate) fn build(
        selection: &ModelSelection,
        mcp_tools: &[McpToolProxy],
        auth_path: &Path,
    ) -> Result<Self> {
        let provider = hedged_provider(selection, auth_path)?;
        let tools = if selection.supports_tools {
            tools(mcp_tools)
        } else {
            ToolRegistry::new()
        };
        Ok(Self::with_tools(provider, tools, selection))
    }

    /// Assemble from already-built parts. The budget is derived from the
    /// registry passed in, which is the registry the kernel will receive.
    pub(crate) fn with_tools(
        provider: Arc<dyn ModelProvider>,
        mut tools: ToolRegistry,
        selection: &ModelSelection,
    ) -> Self {
        // The kernel applies this same baseline in its constructor, so the
        // estimate is measured from the registry the kernel will receive.
        tool::kernel_baseline(&mut tools, provider.capabilities().vision);
        let budget = runtime_core::ContextBudget::new(
            selection.context_capacity(),
            selection.max_output_tokens,
            runtime_core::estimate_tool_schema_tokens(&tools),
        );
        Self {
            provider,
            tools,
            budget,
            codex_auth: selection.codex_auth.clone(),
        }
    }

    /// Schema cost of the exact registry this run will send.
    #[must_use]
    pub(crate) fn tool_schema_tokens(&self) -> usize {
        self.budget.tool_schema_tokens
    }

    #[must_use]
    pub(crate) fn tool_names(&self) -> Vec<String> {
        self.tools.names().into_iter().map(str::to_owned).collect()
    }

    /// Consume the run's parts into the kernel. Child models are additional
    /// configured providers, so each is still built at most once per run.
    pub(crate) fn into_kernel(
        self,
        approval: Arc<dyn ApprovalPolicy>,
        mut messages: Vec<Message>,
        auth_path: &Path,
    ) -> Result<AgentKernel> {
        // The runtime prompt is injected by the kernel itself: every run gets
        // the runtime identity, the per-capability guidance owned by the
        // registered tools, the full environment snapshot and the coding
        // execution policy. There is no global "tool use strategy" prompt and
        // no mode switch — AX is a coding execution harness by construction.
        if let Some(ssh) = tool::SshTool::from_env()? {
            messages.insert(0, Message::system(ssh.instructions()));
        }
        let config = AxConfig::load()?;
        let mut kernel = AgentKernel::new(self.provider, self.tools, approval)
            .with_messages(messages);
        for (name, child) in config.child_models {
            let child_selection = model_selection::selection_for_provider_id(
                &child.provider,
                Some(child.model),
                child
                    .reasoning_effort
                    .as_deref()
                    .and_then(model::ReasoningEffort::parse),
                self.codex_auth.clone(),
            )?;
            kernel.register_child_model(name, build_provider(&child_selection, auth_path)?);
        }
        kernel.configure_retry(config.retry);
        kernel.configure_context_pool(config.context_pool);
        kernel.constrain_permissions(config.permissions);
        Ok(kernel)
    }
}

/// Single-run kernel assembly: `Runtime::build` once, then `into_kernel`.
pub(crate) fn kernel(
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    messages: Vec<Message>,
    mcp_tools: &[McpToolProxy],
    auth_path: &Path,
) -> Result<AgentKernel> {
    Runtime::build(selection, mcp_tools, auth_path)?.into_kernel(approval, messages, auth_path)
}

/// The primary provider plus, in Fast mode, the hedge alternates around it.
fn hedged_provider(selection: &ModelSelection, auth_path: &Path) -> Result<Arc<dyn ModelProvider>> {
    let primary = build_provider(selection, auth_path)?;
    let inference = AxConfig::load().ok().and_then(|config| config.inference);
    Ok(match inference {
        Some(config) if config.mode == InferenceMode::Fast => {
            let fast = config.fast.unwrap_or_default();
            let alternates = hedge_alternates(selection, auth_path);
            Arc::new(HedgingProvider::new(
                primary,
                alternates,
                HedgeConfig {
                    threshold_override: fast.hedge_threshold_ms.map(Duration::from_millis),
                    max_parallel: fast.max_parallel.unwrap_or(2).max(1),
                },
            ))
        }
        _ => primary,
    })
}
/// Other configured providers that serve the same model. The hedge secondary
/// picks the best-ranked one at trigger time; an empty list means the hedge
/// fires a second request at the primary itself, which the design allows.
pub(crate) fn hedge_alternates(
    selection: &ModelSelection,
    auth_path: &Path,
) -> Vec<Arc<dyn ModelProvider>> {
    let mut alternates = Vec::new();
    for provider_id in model_selection::detect_configured_providers(selection.codex_auth.as_ref()) {
        if provider_id == selection.provider_id {
            continue;
        }
        let serves_model = model_selection::local_catalog_models(&provider_id)
            .iter()
            .any(|model| model.id == selection.model);
        if !serves_model {
            continue;
        }
        let Ok(alt_selection) = model_selection::selection_for_provider_id(
            &provider_id,
            Some(selection.model.clone()),
            selection.reasoning_effort,
            selection.codex_auth.clone(),
        ) else {
            continue;
        };
        if let Ok(provider) = build_provider(&alt_selection, auth_path) {
            alternates.push(provider);
        }
    }
    alternates
}
/// Builds the concrete provider for a selection. Called once for the primary
/// and, in Fast mode, once per hedge alternate.
pub(crate) fn build_provider(
    selection: &ModelSelection,
    auth_path: &Path,
) -> Result<Arc<dyn ModelProvider>> {
    let auth = AuthStorage::new(auth_path);
    let provider: Arc<dyn ModelProvider> = match selection.provider {
        ProviderKind::Deepseek => {
            let key = auth
                .resolve_api_key("deepseek", "DEEPSEEK_API_KEY")?
                .ok_or_else(|| {
                    anyhow!("DeepSeek is not configured; open /model and press A to add an API key")
                })?;
            let mut config = model::deepseek_compatible_config(Some(selection.model.clone()), key);
            if let Some(context_window) = selection.context_window {
                config.context_window = context_window;
            }
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiCompatibleProvider::new(config))
        }
        ProviderKind::Openai => {
            let key = auth
                .resolve_api_key("openai", "OPENAI_API_KEY")?
                .ok_or_else(|| {
                    anyhow!("OpenAI is not configured; open /model and press A to add an API key")
                })?;
            let mut config = OpenAiConfig::from_api_key(Some(selection.model.clone()), key);
            if let Some(context_window) = selection.context_window {
                config.context_window = context_window;
            }
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiProvider::new(config))
        }
        ProviderKind::Codex => {
            let mut config = if let Some(path) = selection.codex_auth.clone() {
                // Explicit compatibility import only; AX-owned logins live in
                // `~/.ax/auth.json` like pi's provider-scoped store.
                OpenAiConfig::from_codex_auth(Some(selection.model.clone()), Some(path))?
            } else {
                let credential = auth
                    .resolve_oauth("openai-codex")?
                    .ok_or_else(|| anyhow!("OpenAI Codex is not configured; run /login"))?;
                OpenAiConfig::from_oauth(
                    Some(selection.model.clone()),
                    credential.access,
                    credential.account_id,
                )
            };
            if let Some(context_window) = selection.context_window {
                config.context_window = context_window;
            }
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiProvider::new(config))
        }
        ProviderKind::Workbuddy => {
            let region =
                model::workbuddy::WorkBuddyRegion::from_provider_id(&selection.provider_id)
                    .ok_or_else(|| {
                        anyhow!("unknown WorkBuddy region: {}", selection.provider_id)
                    })?;
            let mut provider = model::workbuddy::WorkBuddyProvider::for_region(
                auth,
                selection.model.clone(),
                selection.context_capacity(),
                region,
            )?;
            provider.set_limits(selection.max_output_tokens, selection.reasoning_effort);
            Arc::new(provider)
        }
        ProviderKind::Compatible | ProviderKind::Native => {
            let spec = model::provider(&selection.provider_id)
                .ok_or_else(|| anyhow!("unknown provider: {}", selection.provider_id))?;
            if let Some(reason) = model::provider_configuration_reason(spec.id) {
                return Err(anyhow!("{reason}"));
            }
            let key = match spec.environment {
                Some(environment) => auth.resolve_api_key(spec.id, environment)?,
                None => None,
            };
            let mut config = model::NativeConfig::new(
                spec.id,
                selection.model.clone(),
                key,
                selection.context_capacity(),
            );
            // Native catalog endpoints are protocol bases; compatible ones are
            // full Chat Completions URLs. Built-in vendor routing wins.
            config.base_url =
                model::provider_chat_endpoint(spec.id).or_else(|| selection.endpoint.clone());
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            model::provider_adapter(config)?
        }
    };
    Ok(provider)
}
pub(crate) fn execution_budget(cli: &Cli) -> runtime_core::ExecutionBudget {
    runtime_core::ExecutionBudget {
        max_steps: cli.max_steps,
        max_tool_calls: cli.max_tool_calls,
        turn_timeout_secs: cli.turn_timeout_secs,
        tool_timeout_secs: cli.tool_timeout_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::AllowAll;

    struct Noop;

    #[async_trait::async_trait]
    impl ModelProvider for Noop {
        fn name(&self) -> &'static str {
            "noop"
        }
        fn model_id(&self) -> &'static str {
            "noop"
        }
        fn context_window(&self) -> usize {
            100_000
        }
        async fn complete(
            &self,
            _: model::ModelRequest,
        ) -> Result<model::ModelResponse, model::ModelError> {
            Ok(model::ModelResponse::default())
        }
    }

    fn selection() -> ModelSelection {
        ModelSelection {
            provider: ProviderKind::Compatible,
            provider_id: "builder-test".into(),
            endpoint: None,
            model: "noop".into(),
            codex_auth: None,
            context_window: Some(100_000),
            max_output_tokens: Some(1_000),
            reasoning_effort: None,
            supports_tools: true,
        }
    }

    #[test]
    fn the_budget_is_measured_from_the_registry_the_kernel_receives() {
        let mut registry = ToolRegistry::new();
        registry.register(tool::FilesystemTool);
        registry.register(tool::ShellTool);
        let selection = selection();
        let plan = Runtime::with_tools(Arc::new(Noop), registry, &selection);
        let estimate = plan.tool_schema_tokens();
        // into_kernel consumes the same registry; the estimate cannot describe a
        // different tool set than the one that is sent.
        let kernel = plan
            .into_kernel(Arc::new(AllowAll), Vec::new(), Path::new("."))
            .unwrap();
        assert_eq!(kernel.tool_schema_tokens(), estimate);
        assert!(kernel.has_tool("filesystem") && kernel.has_tool("shell"));
        // A different registry instance has a different size: the estimate is
        // not derived from some canonical second build.
        assert_ne!(
            runtime_core::estimate_tool_schema_tokens(&ToolRegistry::new()),
            estimate
        );
    }

    #[test]
    fn adding_a_tool_before_build_changes_the_estimate() {
        let selection = selection();
        let base = Runtime::with_tools(Arc::new(Noop), ToolRegistry::new(), &selection);
        let mut registry = ToolRegistry::new();
        registry.register(tool::ShellTool);
        let bigger = Runtime::with_tools(Arc::new(Noop), registry, &selection);
        assert!(bigger.tool_schema_tokens() > base.tool_schema_tokens());
    }
}
