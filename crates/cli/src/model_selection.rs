//! Unified model-selection resolution for the CLI and the TUI.
//!
//! Resolution order — all local, never a network request:
//!   1. explicit `--provider` / `--model` flags;
//!   2. the last successfully selected provider/model in `~/.ax/config.json`;
//!   3. locally detected configured providers (AX auth storage + environment);
//!   4. a single configured provider is selected automatically;
//!   5. multiple providers open the TUI `/model` picker;
//!   6. no provider opens the TUI login flow;
//!   7. non-interactive `run` / `agents` modes return an explicit error for
//!      the ambiguous cases above.
//!
//! Model ids are resolved against the local `~/.ax/models` catalog (the
//! per-provider refresh cache first, then the bundled `pi-catalog.json`
//! directory); the provider fallback model constant is only the last resort.

use std::{fs, path::Path, path::PathBuf};

use anyhow::{Result, anyhow};
use clap::ValueEnum;
use model::{
    DEEPSEEK_FALLBACK_MODEL, ModelInfo, OPENAI_FALLBACK_MODEL, ProviderProtocol, ReasoningEffort,
    provider, provider_chat_endpoint, provider_unsupported_reason,
};
use serde::Deserialize;

use crate::{
    args::Cli,
    config::{AxConfig, ModelConfig, ax_home},
    providers::is_supported_provider,
};

/// Outcome of [`resolve_model_selection`]. Interactive callers decide how to
/// proceed for [`Multiple`](Self::Multiple) / [`None`](Self::None); the
/// non-interactive `run` / `agents` modes turn them into explicit errors.
#[derive(Clone, Debug)]
pub enum ModelResolution {
    /// A concrete model selection is available.
    Resolved(ModelSelection),
    /// Several providers are configured; ask the user which one to use.
    Multiple(Vec<String>),
    /// No provider is configured; open the login/setup flow.
    None,
}

/// Resolves the model selection for this invocation using only local state.
///
/// # Errors
///
/// Returns an error when an explicit selection cannot be resolved (unknown
/// provider, no catalog for an OpenAI-compatible provider, an ambiguous
/// bare `--model`, ...).
pub fn resolve_model_selection(cli: &Cli) -> Result<ModelResolution> {
    // 1. Explicit flags win.
    if let Some(provider) = cli.provider.as_deref() {
        let mut selection =
            selection_for_provider_arg(provider, cli.model.clone(), cli.codex_auth.clone())?;
        apply_cli_overrides(&mut selection, cli)?;
        return Ok(ModelResolution::Resolved(selection));
    }

    // 2. The last successfully selected model, when its provider is still
    //    configured. An explicit `--model` overrides the persisted one.
    let config = AxConfig::load()?;
    if let Some(model_config) = config.model_config()
        && is_configured(&model_config.provider, cli.codex_auth.as_ref())
    {
        let model = cli
            .model
            .clone()
            .unwrap_or_else(|| model_config.model.clone());
        let effort = model_config
            .reasoning_effort
            .as_deref()
            .and_then(ReasoningEffort::parse);
        let mut selection = selection_for_provider_id(
            &model_config.provider,
            Some(model),
            effort,
            cli.codex_auth.clone(),
        )?;
        apply_cli_overrides(&mut selection, cli)?;
        return Ok(ModelResolution::Resolved(selection));
    }

    // 3. Local detection: AX auth storage + environment variables, no network.
    let configured = detect_configured_providers(cli.codex_auth.as_ref());

    // A bare `--model` can still locate its provider locally.
    if let Some(model) = cli.model.as_deref()
        && let Some(selection) = selection_from_bare_model(model, &configured, cli)?
    {
        return Ok(ModelResolution::Resolved(selection));
    }

    // 4/5/6. The caller decides: auto-select the single provider, open the
    // picker or the login flow in the TUI, or error in non-interactive modes.
    match configured.len() {
        1 => {
            let mut selection =
                selection_for_provider_id(&configured[0], None, None, cli.codex_auth.clone())?;
            apply_cli_overrides(&mut selection, cli)?;
            Ok(ModelResolution::Resolved(selection))
        }
        0 => Ok(ModelResolution::None),
        _ => Ok(ModelResolution::Multiple(configured)),
    }
}

/// Non-interactive `run` / `agents` modes: any ambiguity is an explicit error.
///
/// # Errors
///
/// Returns a clear message when several providers are configured or none is.
pub fn require_resolved(cli: &Cli) -> Result<ModelSelection> {
    match resolve_model_selection(cli)? {
        ModelResolution::Resolved(selection) => Ok(selection),
        ModelResolution::Multiple(providers) => Err(anyhow!(
            "Multiple model providers are configured ({}). Pass --provider <name> or run 'ax tui' to select one.",
            providers.join(", ")
        )),
        ModelResolution::None => Err(anyhow!(
            "No model provider is configured. Set a provider API key (e.g. DEEPSEEK_API_KEY), sign in with 'ax tui' → /login, or pass --provider."
        )),
    }
}

/// Persists a successfully applied model selection to `~/.ax/config.json`.
///
/// # Errors
///
/// Returns an error when the config cannot be loaded or written.
pub(crate) fn persist_model_selection(selection: &ModelSelection) -> Result<()> {
    let mut config = AxConfig::load()?;
    config.model = Some(ModelConfig {
        provider: selection.provider_id.clone(),
        model: selection.model.clone(),
        reasoning_effort: selection.reasoning_effort.map(|effort| effort.to_string()),
    });
    config.save()
}

/// Providers with usable credentials. Delegates to [`crate::providers`], the
/// single shared definition of "configured" also used by the TUI catalog
/// refresh, so CLI startup and `/model` never disagree.
pub(crate) fn detect_configured_providers(codex_auth: Option<&PathBuf>) -> Vec<String> {
    crate::providers::configured_providers(codex_auth)
}

/// Local model catalog for a provider: the per-provider refresh cache
/// (`~/.ax/models/<provider>.json`) first, then the bundled `pi-catalog.json`
/// directory. Pure file reads — never a network request.
pub(crate) fn local_catalog_models(provider_id: &str) -> Vec<ModelInfo> {
    local_catalog_models_in(&ax_home().join("models"), provider_id)
}

fn local_catalog_models_in(models_dir: &Path, provider_id: &str) -> Vec<ModelInfo> {
    if let Some(models) = load_provider_cache(models_dir, provider_id) {
        return models;
    }
    // Older AX builds cached codex models under `codex.json`.
    if provider_id == "openai-codex"
        && let Some(models) = load_provider_cache(models_dir, "codex")
    {
        return models;
    }
    let pi_path = models_dir.join("pi-catalog.json");
    if let Ok(contents) = fs::read(&pi_path)
        && let Ok(all) = serde_json::from_slice::<Vec<ModelInfo>>(&contents)
    {
        let matched = all
            .into_iter()
            .filter(|model| model.provider == provider_id)
            .collect::<Vec<_>>();
        if !matched.is_empty() {
            return matched;
        }
    }
    model::builtin_models(provider_id)
}

/// A placeholder selection shown in the TUI while the user is being asked to
/// pick a provider, so the status line and startup card have a model label.
pub(crate) fn placeholder_for(providers: &[String], codex_auth: Option<PathBuf>) -> ModelSelection {
    providers
        .first()
        .and_then(|id| selection_for_provider_id(id, None, None, codex_auth).ok())
        .unwrap_or_else(unconfigured_selection)
}

/// Selection used when no provider is configured yet; replaced as soon as the
/// user completes login or picks a model.
#[must_use]
pub(crate) fn unconfigured_selection() -> ModelSelection {
    ModelSelection {
        provider: ProviderKind::Deepseek,
        provider_id: String::new(),
        endpoint: None,
        model: "(not configured)".to_owned(),
        codex_auth: None,
        context_window: Some(64_000),
        max_output_tokens: None,
        reasoning_effort: None,
        supports_tools: true,
    }
}

// ---- helpers ---------------------------------------------------------------

/// `--provider <value>`: one of the built-in aliases, or a concrete provider
/// id from the catalog.
///
/// Vendors can appear several times in the catalog — Xiaomi lists a plain
/// entry plus CN/SGP/AMS token-plan regions that share model ids but not
/// endpoints — so the id has to be nameable; `compatible` alone cannot tell
/// them apart once more than one is credentialed.
fn selection_for_provider_arg(
    value: &str,
    model: Option<String>,
    codex_auth: Option<PathBuf>,
) -> Result<ModelSelection> {
    if let Ok(kind) = ProviderKind::from_str(value, true) {
        return selection_for_kind(kind, model, codex_auth);
    }
    let spec = provider(value).ok_or_else(|| {
        anyhow!(
            "unknown provider '{value}'; use one of deepseek, openai, codex, compatible or a provider id from the model catalog"
        )
    })?;
    if let Some(reason) = provider_unsupported_reason(spec.id) {
        return Err(anyhow!("provider '{}' cannot be used: {reason}", spec.id));
    }
    selection_for_provider_id(spec.id, model, None, codex_auth)
}

fn selection_for_kind(
    kind: ProviderKind,
    model: Option<String>,
    codex_auth: Option<PathBuf>,
) -> Result<ModelSelection> {
    let provider_id = match kind {
        ProviderKind::Deepseek => "deepseek".to_owned(),
        ProviderKind::Openai => "openai".to_owned(),
        ProviderKind::Codex => "openai-codex".to_owned(),
        ProviderKind::Workbuddy => "workbuddy".to_owned(),
        ProviderKind::Compatible => {
            let configured = detect_configured_providers(codex_auth.as_ref());
            let compatible = configured
                .iter()
                .filter(|id| {
                    !matches!(
                        id.as_str(),
                        "deepseek" | "openai" | "openai-codex" | "workbuddy" | "workbuddy-cn"
                    )
                })
                .cloned()
                .collect::<Vec<_>>();
            match compatible.len() {
                1 => compatible[0].clone(),
                0 => {
                    return Err(anyhow!(
                        "--provider compatible needs an OpenAI-compatible provider configured; add its API key to ~/.ax/auth.json or set its environment variable"
                    ));
                }
                _ => {
                    return Err(anyhow!(
                        "Multiple OpenAI-compatible providers are configured ({}); open the TUI /model picker to choose one",
                        compatible.join(", ")
                    ));
                }
            }
        }
    };
    selection_for_provider_id(&provider_id, model, None, codex_auth)
}

pub(crate) fn selection_for_provider_id(
    provider_id: &str,
    model: Option<String>,
    effort: Option<ReasoningEffort>,
    codex_auth: Option<PathBuf>,
) -> Result<ModelSelection> {
    let kind = provider_kind_for(provider_id)?;
    let models = local_catalog_models(provider_id);
    let model_id = match model {
        Some(model) => model,
        None => default_model_for(provider_id, &models)?,
    };
    let info = models.iter().find(|model| model.id == model_id);
    let context_window =
        info.and_then(|model| (model.context_window > 0).then_some(model.context_window));
    let max_output_tokens = info.and_then(|model| model.max_output_tokens);
    let endpoint = provider_chat_endpoint(provider_id)
        .or_else(|| info.and_then(|model| model.endpoint.clone()));
    let supports_tools = info.is_none_or(|model| model.supports_tools);
    let reasoning_effort = effort.or_else(|| info.and_then(|model| model.default_reasoning_effort));
    Ok(ModelSelection {
        provider: kind,
        provider_id: provider_id.to_owned(),
        endpoint,
        model: model_id,
        codex_auth,
        context_window,
        max_output_tokens,
        reasoning_effort,
        supports_tools,
    })
}

/// Catalog entries a bare model id could mean, narrowed to providers this
/// machine has credentials for.
///
/// A bare id can match several providers: Xiaomi and its CN/SGP/AMS
/// token-plan regions list the same model ids under different endpoints, so
/// an unfiltered match would happily pick the plain `xiaomi` entry — and its
/// endpoint — for a credential that only works on a token-plan host.
fn bare_model_candidates(
    model: &str,
    catalog: &[ModelInfo],
    configured: &[String],
) -> Vec<ModelInfo> {
    let candidates = catalog
        .iter()
        .filter(|info| info.id == model && is_supported_provider(&info.provider))
        .cloned()
        .collect::<Vec<_>>();
    if candidates.len() < 2 {
        return candidates;
    }
    let credentialed = candidates
        .iter()
        .filter(|info| configured.iter().any(|id| id == &info.provider))
        .cloned()
        .collect::<Vec<_>>();
    if credentialed.is_empty() {
        candidates
    } else {
        credentialed
    }
}

/// Resolves a bare `--model` (no `--provider`) from the local catalog,
/// falling back to the sole configured provider.
fn selection_from_bare_model(
    model: &str,
    configured: &[String],
    cli: &Cli,
) -> Result<Option<ModelSelection>> {
    let candidates = bare_model_candidates(model, &bundled_catalog(), configured);
    if candidates.len() == 1 {
        let mut selection = selection_from_info(&candidates[0])?;
        selection.codex_auth.clone_from(&cli.codex_auth);
        apply_cli_overrides(&mut selection, cli)?;
        return Ok(Some(selection));
    }
    if candidates.len() > 1 {
        let providers = candidates
            .iter()
            .map(|info| info.provider.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(anyhow!(
            "model '{model}' is available from multiple providers ({providers}); pass --provider <name>"
        ));
    }
    if configured.len() == 1 {
        let mut selection = selection_for_provider_id(
            &configured[0],
            Some(model.to_owned()),
            None,
            cli.codex_auth.clone(),
        )?;
        apply_cli_overrides(&mut selection, cli)?;
        return Ok(Some(selection));
    }
    Err(anyhow!(
        "cannot determine the provider for model '{model}'; pass --provider <name>"
    ))
}

fn selection_from_info(info: &ModelInfo) -> Result<ModelSelection> {
    let provider_id = info.provider.clone();
    let kind = provider_kind_for(&provider_id)?;
    Ok(ModelSelection {
        provider: kind,
        provider_id: provider_id.clone(),
        endpoint: provider_chat_endpoint(&provider_id).or_else(|| info.endpoint.clone()),
        model: info.id.clone(),
        codex_auth: None,
        context_window: (info.context_window > 0).then_some(info.context_window),
        max_output_tokens: info.max_output_tokens,
        reasoning_effort: info.default_reasoning_effort,
        supports_tools: info.supports_tools,
    })
}

fn provider_kind_for(provider_id: &str) -> Result<ProviderKind> {
    match provider_id {
        "deepseek" => Ok(ProviderKind::Deepseek),
        "openai" => Ok(ProviderKind::Openai),
        "openai-codex" | "codex" => Ok(ProviderKind::Codex),
        "workbuddy" | "workbuddy-cn" => Ok(ProviderKind::Workbuddy),
        id if provider(id)
            .is_some_and(|spec| spec.protocol == ProviderProtocol::OpenAiCompatible) =>
        {
            Ok(ProviderKind::Compatible)
        }
        id => Err(anyhow!(
            "'{id}' has no enabled adapter in this AX build; use the TUI /model picker"
        )),
    }
}

/// Chooses the default model for a provider: the provider's fallback model
/// when the catalog lists it, otherwise the first catalog entry.
fn default_model_for(provider_id: &str, models: &[ModelInfo]) -> Result<String> {
    let fallback = fallback_model_for(provider_id);
    if let Some(matched) = models.iter().find(|model| model.id == fallback) {
        return Ok(matched.id.clone());
    }
    if let Some(first) = models.first() {
        return Ok(first.id.clone());
    }
    if !fallback.is_empty() {
        return Ok(fallback.to_owned());
    }
    Err(anyhow!(
        "no cached model catalog for '{provider_id}'; pass --model <model-id>, or open the TUI /model picker to discover its models"
    ))
}

fn fallback_model_for(provider_id: &str) -> &'static str {
    match provider_id {
        "deepseek" => DEEPSEEK_FALLBACK_MODEL,
        "openai" | "openai-codex" | "codex" => OPENAI_FALLBACK_MODEL,
        _ => "",
    }
}

fn is_configured(provider_id: &str, codex_auth: Option<&PathBuf>) -> bool {
    detect_configured_providers(codex_auth)
        .iter()
        .any(|id| id == provider_id)
}

fn apply_cli_overrides(selection: &mut ModelSelection, cli: &Cli) -> Result<()> {
    if let Some(window) = cli.context_window {
        selection.context_window = Some(window.get());
    }
    if let Some(value) = cli.reasoning_effort.as_deref() {
        let effort = ReasoningEffort::parse(value).ok_or_else(|| {
            anyhow!(
                "invalid --reasoning-effort '{value}' (expected low, medium, high, xhigh or max)"
            )
        })?;
        selection.reasoning_effort = Some(effort);
    }
    Ok(())
}

fn load_provider_cache(models_dir: &Path, provider_id: &str) -> Option<Vec<ModelInfo>> {
    let cache_path = models_dir.join(format!("{provider_id}.json"));
    let contents = fs::read(&cache_path).ok()?;
    let cache = serde_json::from_slice::<ProviderCache>(&contents).ok()?;
    (!cache.models.is_empty()).then_some(cache.models)
}

/// The bundled static directory listing every known model.
fn bundled_catalog() -> Vec<ModelInfo> {
    let pi_path = ax_home().join("models").join("pi-catalog.json");
    let Ok(contents) = fs::read(&pi_path) else {
        return Vec::new();
    };
    serde_json::from_slice::<Vec<ModelInfo>>(&contents).unwrap_or_default()
}

#[derive(Deserialize)]
struct ProviderCache {
    #[allow(dead_code)]
    saved_at_unix: u64,
    models: Vec<ModelInfo>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use clap::Parser;

    use super::*;

    /// `--provider` used to accept only `deepseek|openai|codex|compatible`,
    /// which made a vendor's regional entries unreachable from the CLI: with
    /// `xiaomi` and `xiaomi-token-plan-cn` both credentialed, `compatible` was
    /// ambiguous and every run fell back to whichever the config had last.
    #[test]
    fn provider_flag_accepts_a_catalog_provider_id() {
        for value in ["deepseek", "codex", "compatible", "xiaomi-token-plan-cn"] {
            let cli = Cli::try_parse_from(["ax", "--provider", value, "tui"]).unwrap();
            assert_eq!(cli.provider.as_deref(), Some(value));
        }
    }

    fn info(id: &str, provider: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            display_name: id.to_owned(),
            provider: provider.to_owned(),
            context_window: 128_000,
            max_output_tokens: None,
            reasoning_efforts: Vec::new(),
            default_reasoning_effort: None,
            supports_tools: true,
            endpoint: None,
        }
    }

    #[test]
    fn canonical_endpoint_overrides_stale_protocol_and_path() {
        for (provider, stale) in [
            (
                "minimax",
                "https://api.minimax.io/anthropic/v1/chat/completions",
            ),
            (
                "fireworks",
                "https://api.fireworks.ai/inference/chat/completions",
            ),
        ] {
            let mut entry = info("test-model", provider);
            entry.endpoint = Some(stale.to_owned());
            let selected = selection_from_info(&entry).unwrap();
            assert_eq!(selected.endpoint, provider_chat_endpoint(provider));
            assert_ne!(selected.endpoint.as_deref(), Some(stale));
        }
    }

    #[test]
    fn clean_install_has_models_without_home_catalog() {
        let missing = std::env::temp_dir().join("ax-no-catalog-installed");
        let models = local_catalog_models_in(&missing, "minimax");
        assert!(!models.is_empty());
        assert!(
            models
                .iter()
                .all(|item| item.provider == "minimax" && item.supports_tools)
        );
    }

    /// The Xiaomi regions share model ids, so a bare id must be narrowed to a
    /// provider AX can actually call instead of landing on the plain entry.
    #[test]
    fn bare_model_prefers_a_credentialed_provider() {
        let catalog = vec![
            info("mimo-v2.5", "xiaomi"),
            info("mimo-v2.5", "xiaomi-token-plan-cn"),
            info("mimo-v2.5", "xiaomi-token-plan-sgp"),
        ];
        let candidates =
            bare_model_candidates("mimo-v2.5", &catalog, &["xiaomi-token-plan-cn".to_owned()]);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provider, "xiaomi-token-plan-cn");

        // Without a credential the ambiguity stays visible to the caller.
        assert_eq!(bare_model_candidates("mimo-v2.5", &catalog, &[]).len(), 3);
        assert_eq!(
            bare_model_candidates("mimo-v2.5", &catalog, &["groq".to_owned()]).len(),
            3
        );
    }

    #[test]
    fn reasoning_effort_cli_flag_overrides_selection() {
        let mut selection = unconfigured_selection();
        let cli = Cli::try_parse_from(["ax", "--reasoning-effort", "high", "tui"]).unwrap();
        apply_cli_overrides(&mut selection, &cli).unwrap();
        assert_eq!(selection.reasoning_effort, Some(ReasoningEffort::High));
        let cli = Cli::try_parse_from(["ax", "--reasoning-effort", "bogus", "tui"]).unwrap();
        assert!(apply_cli_overrides(&mut selection, &cli).is_err());
    }

    #[test]
    fn provider_cache_precedes_bundled_catalog() {
        let root = std::env::temp_dir().join(format!("ax-model-dir-cache-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("deepseek.json"),
            r#"{"saved_at_unix":1,"models":[{"id":"cached-model","display_name":"Cached","provider":"deepseek","context_window":128000,"reasoning_efforts":[],"default_reasoning_effort":null,"supports_tools":true}]}"#,
        )
        .unwrap();
        fs::write(
            root.join("pi-catalog.json"),
            r#"[{"id":"bundled-model","display_name":"Bundled","provider":"deepseek","context_window":1000000,"reasoning_efforts":[],"default_reasoning_effort":null,"supports_tools":true}]"#,
        )
        .unwrap();

        let models = local_catalog_models_in(&root, "deepseek");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "cached-model");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bundled_catalog_backs_unknown_provider() {
        let root =
            std::env::temp_dir().join(format!("ax-model-dir-bundled-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("pi-catalog.json"),
            r#"[{"id":"groq-model","display_name":"Groq Model","provider":"groq","context_window":128000,"reasoning_efforts":[],"default_reasoning_effort":null,"supports_tools":true}]"#,
        )
        .unwrap();

        let models = local_catalog_models_in(&root, "groq");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "groq-model");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_model_prefers_fallback_when_listed() {
        let models = vec![
            ModelInfo {
                id: "first".to_owned(),
                display_name: "First".to_owned(),
                provider: "deepseek".to_owned(),
                context_window: 128_000,
                max_output_tokens: None,
                reasoning_efforts: Vec::new(),
                default_reasoning_effort: None,
                supports_tools: true,
                endpoint: None,
            },
            ModelInfo {
                id: DEEPSEEK_FALLBACK_MODEL.to_owned(),
                display_name: "Fallback".to_owned(),
                provider: "deepseek".to_owned(),
                context_window: 128_000,
                max_output_tokens: None,
                reasoning_efforts: Vec::new(),
                default_reasoning_effort: None,
                supports_tools: true,
                endpoint: None,
            },
        ];
        assert_eq!(
            default_model_for("deepseek", &models).unwrap(),
            DEEPSEEK_FALLBACK_MODEL
        );
        assert_eq!(
            default_model_for("deepseek", &[]).unwrap(),
            DEEPSEEK_FALLBACK_MODEL
        );
        assert!(default_model_for("groq", &[]).is_err());
    }

    #[test]
    fn config_round_trip() {
        // Write through a temp path so the real user config is untouched.
        let root = std::env::temp_dir().join(format!("ax-config-{}", std::process::id()));
        let path = root.join("config.json");
        let config = AxConfig {
            verification: runtime_core::VerificationConfig::default(),
            child_models: std::collections::BTreeMap::default(),
            context_pool: runtime_core::ContextPoolPolicy::default(),
            permissions: tool::PermissionProfile::default(),
            retry: model::RetryPolicy::default(),
            sandbox: sandbox::SandboxMode::default(),
            subagent: runtime_core::SubagentConfig::default(),
            execution: crate::config::ExecutionConfig::default(),
            model: Some(ModelConfig {
                provider: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                reasoning_effort: Some("high".to_owned()),
            }),
            inference: None,
        };
        config.save_to(&path).unwrap();
        let loaded = AxConfig::load_from(&path).unwrap();
        assert_eq!(loaded.model_config().unwrap().provider, "deepseek");
        assert_eq!(
            loaded.model_config().unwrap().reasoning_effort.as_deref(),
            Some("high")
        );
        assert!(
            AxConfig::load_from(&root.join("missing.json"))
                .unwrap()
                .model_config()
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_toml_config_is_migrated_to_json() {
        let root = std::env::temp_dir().join(format!("ax-config-migrate-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("config.toml"),
            "[model]\nprovider = 'deepseek'\nmodel = 'deepseek-flash'\n",
        )
        .unwrap();
        let loaded = AxConfig::load_from_home(&root).unwrap();
        assert_eq!(loaded.model_config().unwrap().provider, "deepseek");
        assert!(root.join("config.json").is_file());
        assert_eq!(
            AxConfig::load_from_home(&root)
                .unwrap()
                .model_config()
                .unwrap()
                .model,
            "deepseek-flash"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ProviderKind {
    Deepseek,
    Openai,
    Codex,
    Workbuddy,
    Compatible,
}
#[derive(Clone, Debug)]
pub(crate) struct ModelSelection {
    pub(crate) provider: ProviderKind,
    pub(crate) provider_id: String,
    pub(crate) endpoint: Option<String>,
    pub(crate) model: String,
    pub(crate) codex_auth: Option<PathBuf>,
    pub(crate) context_window: Option<usize>,
    pub(crate) max_output_tokens: Option<usize>,
    pub(crate) reasoning_effort: Option<ReasoningEffort>,
    pub(crate) supports_tools: bool,
}
impl ModelSelection {
    pub(crate) const fn context_capacity(&self) -> usize {
        match self.context_window {
            Some(capacity) => capacity,
            None => match self.provider {
                ProviderKind::Deepseek => 64_000,
                ProviderKind::Workbuddy => 128_000,
                ProviderKind::Openai | ProviderKind::Codex | ProviderKind::Compatible => 200_000,
            },
        }
    }
}
