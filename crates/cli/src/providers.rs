//! Single source of truth for which providers have usable local credentials.
//!
//! CLI startup resolution ([`crate::model_selection`]) and the TUI model
//! catalog refresh ([`crate::tui::catalog_refresh`]) previously duplicated
//! this logic and drifted apart: the catalog refresh never checked
//! environment variables, so a provider configured only via
//! `DEEPSEEK_API_KEY` (say) would auto-select at startup but show up as
//! unconfigured in `/model`. Both now call the functions here, which
//! recognize AX's own `auth.json`, conventional environment variables, and
//! (for `openai-codex` only) an explicitly supplied legacy auth file. This
//! never makes a network request.

use std::path::PathBuf;

use model::{AuthStorage, OpenAiConfig, PROVIDERS, provider_supported};

/// Providers AX's built-in adapters can actually drive.
///
/// Delegates to [`model::provider_supported`] so the "can AX drive this" rule
/// has exactly one definition; this module used to re-derive it from the
/// protocol table and the base-URL table on its own.
pub(crate) fn is_supported_provider(provider_id: &str) -> bool {
    provider_supported(provider_id)
}

/// Providers with usable credentials, discovered purely locally: AX auth
/// storage, conventional environment variables, and an explicit legacy Codex
/// auth path. No network request is ever made here.
///
/// This is the unfiltered set. Credentials that belong to a provider AX cannot
/// drive are included on purpose, so the ACP surface can report them as
/// unsupported instead of dropping a saved key without a word.
pub(crate) fn credentialed_providers(codex_auth: Option<&PathBuf>) -> Vec<String> {
    let auth = AuthStorage::new(crate::bootstrap::ax_auth_path());
    let disabled = auth.disabled_provider_ids().unwrap_or_default();
    let mut credentialed = Vec::new();
    for id in auth.provider_ids().unwrap_or_default() {
        push_unique(&mut credentialed, &id);
    }
    for spec in PROVIDERS {
        if disabled.iter().any(|id| id == spec.id) {
            continue;
        }
        if model::ambient_credentials_configured(spec.id) {
            push_unique(&mut credentialed, spec.id);
        }
        if let Some(environment) = spec.environment
            && std::env::var(environment).is_ok_and(|value| !value.is_empty())
        {
            push_unique(&mut credentialed, spec.id);
        }
    }
    if !disabled.iter().any(|id| id == "openai-codex")
        && codex_auth.is_some()
        && OpenAiConfig::from_codex_auth(None, codex_auth.cloned()).is_ok()
    {
        push_unique(&mut credentialed, "openai-codex");
    }
    credentialed
}

/// The credentialed subset AX can actually drive. This is what model
/// selection and the model catalog are allowed to offer.
pub(crate) fn configured_providers(codex_auth: Option<&PathBuf>) -> Vec<String> {
    credentialed_providers(codex_auth)
        .into_iter()
        .filter(|id| is_supported_provider(id))
        .collect()
}

fn push_unique(configured: &mut Vec<String>, provider_id: &str) {
    if !configured.iter().any(|id| id == provider_id) {
        configured.push(provider_id.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_provider_filter_matches_builtin_adapters() {
        assert!(is_supported_provider("deepseek"));
        assert!(is_supported_provider("openai"));
        assert!(is_supported_provider("openai-codex"));
        assert!(is_supported_provider("groq"));
        assert!(is_supported_provider("workbuddy"));
        assert!(is_supported_provider("workbuddy-cn"));
        assert!(is_supported_provider("anthropic"));
        assert!(is_supported_provider("amazon-bedrock"));
        assert!(!is_supported_provider("does-not-exist"));
    }

    /// CLI startup (`model_selection::detect_configured_providers`) and the
    /// TUI catalog refresh (`tui::catalog_refresh::configured_providers`)
    /// both delegate here, so they can never disagree about what counts as
    /// configured (the inconsistency this module fixes).
    #[test]
    fn model_selection_agrees_with_the_shared_definition() {
        assert_eq!(
            configured_providers(None),
            crate::model_selection::detect_configured_providers(None)
        );
    }
}
