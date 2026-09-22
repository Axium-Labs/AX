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

use model::{AuthStorage, OpenAiConfig, PROVIDERS, ProviderProtocol, provider, provider_base_url};

/// Providers AX's built-in adapters can actually drive.
pub(crate) fn is_supported_provider(provider_id: &str) -> bool {
    provider(provider_id).is_some_and(|spec| {
        matches!(
            spec.protocol,
            ProviderProtocol::OpenAiCompatible | ProviderProtocol::OpenAiResponses
        ) && (matches!(spec.id, "openai" | "openai-codex") || provider_base_url(spec.id).is_some())
    })
}

/// Providers with usable credentials, discovered purely locally: AX auth
/// storage, conventional environment variables, and an explicit legacy Codex
/// auth path. No network request is ever made here.
pub(crate) fn configured_providers(codex_auth: Option<&PathBuf>) -> Vec<String> {
    let auth = AuthStorage::new(crate::ax_auth_path());
    let mut configured = Vec::new();
    for id in auth.provider_ids().unwrap_or_default() {
        if is_supported_provider(&id) {
            push_unique(&mut configured, &id);
        }
    }
    for spec in PROVIDERS {
        if let Some(environment) = spec.environment
            && std::env::var(environment).is_ok_and(|value| !value.is_empty())
            && is_supported_provider(spec.id)
        {
            push_unique(&mut configured, spec.id);
        }
    }
    if codex_auth.is_some() && OpenAiConfig::from_codex_auth(None, codex_auth.cloned()).is_ok() {
        push_unique(&mut configured, "openai-codex");
    }
    configured
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
        assert!(!is_supported_provider("anthropic"));
        assert!(!is_supported_provider("amazon-bedrock"));
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
