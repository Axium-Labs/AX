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
    AuthStorage, HedgeConfig, HedgingProvider, Message, ModelProvider, OpenAiCompatibleConfig,
    OpenAiCompatibleProvider, OpenAiConfig, OpenAiProvider,
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
    for local in [
        Arc::new(ShellTool) as Arc<dyn tool::Tool>,
        Arc::new(FilesystemTool),
        Arc::new(tool::PatchTool),
        Arc::new(tool::SearchTool),
        Arc::new(tool::ViewImageTool::new(root.clone())),
    ] {
        registry.register(tool::SandboxedTool::new(local, root.clone()));
    }
    registry.register(tool::WebTool::new());
    for tool in mcp_tools {
        registry.register(tool.clone());
    }
    registry
}
pub(crate) fn tool_workspace() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_default();
    discover_project_root(&cwd)
}
/// Single source of the context budget available for one turn: reserves room
/// for the reply and the tool schemas that will actually be sent, so history
/// restore, skill instructions, and retrieved memory all share one real
/// accounting of what fits instead of each guessing its own fixed limit.
pub(crate) fn context_budget(
    selection: &ModelSelection,
    mcp_tools: &[McpToolProxy],
) -> runtime_core::ContextBudget {
    let tool_schema_tokens = runtime_core::estimate_tool_schema_tokens(&tools(mcp_tools));
    runtime_core::ContextBudget::new(
        selection.context_capacity(),
        selection.max_output_tokens,
        tool_schema_tokens,
    )
}
pub(crate) fn kernel(
    selection: &ModelSelection,
    approval: Arc<dyn ApprovalPolicy>,
    mut messages: Vec<Message>,
    mcp_tools: &[McpToolProxy],
    auth_path: &Path,
) -> Result<AgentKernel> {
    let primary = build_provider(selection, auth_path)?;
    let inference = AxConfig::load().ok().and_then(|config| config.inference);
    let provider: Arc<dyn ModelProvider> = match inference {
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
    };
    let tool_registry = if selection.supports_tools {
        tools(mcp_tools)
    } else {
        ToolRegistry::new()
    };
    let policy = include_str!("tool_policy.md");
    if !messages.iter().any(|message| message.content == policy) {
        messages.insert(0, Message::system(policy));
    }
    let config = AxConfig::load()?;
    let mut kernel = AgentKernel::new(provider, tool_registry, approval).with_messages(messages);
    for (name, child) in config.child_models {
        let child_selection = model_selection::selection_for_provider_id(
            &child.provider,
            Some(child.model),
            child
                .reasoning_effort
                .as_deref()
                .and_then(model::ReasoningEffort::parse),
            selection.codex_auth.clone(),
        )?;
        kernel.register_child_model(name, build_provider(&child_selection, auth_path)?);
    }
    kernel.configure_retry(config.retry);
    kernel.configure_context_pool(config.context_pool);
    kernel.constrain_permissions(config.permissions);
    Ok(kernel)
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
        ProviderKind::Compatible => {
            let spec = model::provider(&selection.provider_id).ok_or_else(|| {
                anyhow!(
                    "unknown OpenAI-compatible provider: {}",
                    selection.provider_id
                )
            })?;
            let environment = spec
                .environment
                .ok_or_else(|| anyhow!("{} does not use a direct API key", spec.name))?;
            let key = auth
                .resolve_api_key(spec.id, environment)?
                .ok_or_else(|| anyhow!("{} is not configured; run /login", spec.name))?;
            let endpoint = selection
                .endpoint
                .clone()
                .ok_or_else(|| anyhow!("{} catalog did not provide an API endpoint", spec.name))?;
            let mut config = OpenAiCompatibleConfig::new(
                spec.id,
                selection.model.clone(),
                key,
                endpoint,
                selection.context_capacity(),
            );
            config.max_output_tokens = selection.max_output_tokens;
            config.reasoning_effort = selection.reasoning_effort;
            Arc::new(OpenAiCompatibleProvider::new(config))
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
