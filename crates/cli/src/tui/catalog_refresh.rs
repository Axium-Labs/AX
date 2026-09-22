//! Shared, concurrent model-catalog refresh.
//!
//! Ported in principle from pi's `ModelCatalogRefreshCoordinator`: concurrent
//! callers (opening the model picker, resolving `/model <term>`) share one
//! in-flight refresh for the same data directory instead of each firing their
//! own network request. Callers wait on the same result, bounded by a timeout,
//! and fall back to the cached snapshot when a refresh cannot complete in time.
//!
//! Only models from configured providers are ever collected, mirroring pi's
//! `snapshot.available = all.filter(configuredProviders.has(provider))`. A
//! provider counts as configured when it has a credential in AX's own auth
//! store. An explicitly supplied legacy Codex auth path remains supported,
//! but AX never reads another application's default credential location.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use model::{
    AuthStorage, DeepSeekConfig, DeepSeekProvider, ModelInfo, ModelRegistry, OpenAiConfig,
    OpenAiProvider, ProviderProtocol,
};

/// Overall deadline for a full catalog refresh, mirroring pi's 15s timeout.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);

/// Result of a full catalog refresh.
pub struct CatalogRefreshResult {
    pub models: Vec<ModelInfo>,
    /// Providers whose live refresh failed; their cached snapshots were kept.
    pub failed: Vec<String>,
}

struct SharedRefresh {
    notify: tokio::sync::Notify,
    outcome: Mutex<Option<Arc<CatalogRefreshResult>>>,
}

/// In-flight refreshes keyed by data directory.
static ACTIVE: LazyLock<Mutex<HashMap<String, Arc<SharedRefresh>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Start or join a full catalog refresh for `data_dir`.
///
/// Callers share the in-flight refresh for the same data directory; each
/// caller waits on the same result bounded by [`REFRESH_TIMEOUT`]. On timeout
/// the cached snapshot is returned so the UI keeps working with local data.
pub async fn refresh_catalogs(
    data_dir: PathBuf,
    codex_auth: Option<PathBuf>,
) -> Arc<CatalogRefreshResult> {
    let key = data_dir.to_string_lossy().into_owned();
    let shared = {
        let mut active = ACTIVE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = active.get(&key) {
            Arc::clone(existing)
        } else {
            let shared = Arc::new(SharedRefresh {
                notify: tokio::sync::Notify::new(),
                outcome: Mutex::new(None),
            });
            active.insert(key.clone(), Arc::clone(&shared));
            spawn_refresh(
                Arc::clone(&shared),
                key,
                data_dir.clone(),
                codex_auth.clone(),
            );
            shared
        }
    };

    tokio::select! {
        () = shared.notify.notified() => {
            shared
                .outcome
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| fallback(&data_dir, codex_auth.as_ref()))
        }
        () = tokio::time::sleep(REFRESH_TIMEOUT) => {
            fallback(&data_dir, codex_auth.as_ref())
        }
    }
}

fn spawn_refresh(
    shared: Arc<SharedRefresh>,
    key: String,
    data_dir: PathBuf,
    codex_auth: Option<PathBuf>,
) {
    tokio::spawn(async move {
        let outcome = Arc::new(run_refresh(&data_dir, codex_auth).await);
        *shared
            .outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&outcome));
        shared.notify.notify_waiters();
        ACTIVE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&key);
    });
}

/// Providers AX considers configured for the picker. This mirrors pi's
/// `configuredProviders`: credentials explicitly stored by AX. Environment
/// variables and another application's default auth file do not silently add
/// providers to `/model`.
pub(crate) fn configured_providers(_data_dir: &Path, codex_auth: Option<&PathBuf>) -> Vec<String> {
    let auth = AuthStorage::new(crate::ax_auth_path());
    let stored = auth.provider_ids().unwrap_or_default();
    let mut configured = stored
        .into_iter()
        .filter(|provider_id| {
            model::provider(provider_id).is_some_and(|provider| {
                matches!(
                    provider.protocol,
                    ProviderProtocol::OpenAiCompatible | ProviderProtocol::OpenAiResponses
                ) && (matches!(provider.id, "openai" | "openai-codex")
                    || model::provider_base_url(provider.id).is_some())
            })
        })
        .collect::<Vec<_>>();
    if codex_auth.is_some()
        && OpenAiConfig::from_codex_auth(None, codex_auth.cloned()).is_ok()
        && !configured.iter().any(|provider| provider == "openai-codex")
    {
        configured.push("openai-codex".to_owned());
    }
    configured
}

fn codex_provider(
    auth: &AuthStorage,
    legacy_path: Option<PathBuf>,
    model: Option<String>,
) -> Option<OpenAiProvider> {
    let config = if let Some(credential) = auth.resolve_oauth("openai-codex").ok().flatten() {
        OpenAiConfig::from_oauth(model, credential.access, credential.account_id)
    } else {
        OpenAiConfig::from_codex_auth(model, legacy_path).ok()?
    };
    Some(OpenAiProvider::new(config))
}

/// Non-blocking cached snapshot restricted to configured providers, used as
/// the base and as the timeout fallback.
pub(crate) fn cached_snapshot(data_dir: &Path, codex_auth: Option<&PathBuf>) -> Vec<ModelInfo> {
    let registry = ModelRegistry::new(crate::ax_models_dir());
    let auth = AuthStorage::new(crate::ax_auth_path());
    let configured = configured_providers(data_dir, codex_auth);
    let mut models = Vec::new();
    if configured.iter().any(|provider| provider == "deepseek") {
        let key = auth
            .resolve_api_key("deepseek", "DEEPSEEK_API_KEY")
            .ok()
            .flatten()
            .unwrap_or_default();
        let provider = DeepSeekProvider::new(DeepSeekConfig::from_api_key(None, key));
        models.extend(registry.cached(&provider, "cached snapshot").models);
    }
    if configured.iter().any(|provider| provider == "openai") {
        let key = auth
            .resolve_api_key("openai", "OPENAI_API_KEY")
            .ok()
            .flatten()
            .unwrap_or_default();
        let provider = OpenAiProvider::new(OpenAiConfig::from_api_key(None, key));
        models.extend(registry.cached(&provider, "cached snapshot").models);
    }
    if configured.iter().any(|provider| provider == "openai-codex")
        && let Some(provider) = codex_provider(&auth, codex_auth.cloned(), None)
    {
        models.extend(registry.cached(&provider, "cached snapshot").models);
    }
    for provider_id in configured
        .iter()
        .filter(|provider| !matches!(provider.as_str(), "deepseek" | "openai" | "openai-codex"))
    {
        let Some(provider) = compatible_provider(&auth, provider_id) else {
            continue;
        };
        models.extend(registry.cached(&provider, "cached snapshot").models);
    }
    sort_models(&mut models);
    models
}

fn fallback(data_dir: &Path, codex_auth: Option<&PathBuf>) -> Arc<CatalogRefreshResult> {
    Arc::new(CatalogRefreshResult {
        models: cached_snapshot(data_dir, codex_auth),
        failed: Vec::new(),
    })
}

/// Run the full refresh: build the configured-provider cached base, refresh
/// every configured provider concurrently, then replace each provider's models
/// as a whole.
async fn run_refresh(data_dir: &Path, codex_auth: Option<PathBuf>) -> CatalogRefreshResult {
    let registry = ModelRegistry::new(crate::ax_models_dir());
    let auth = AuthStorage::new(crate::ax_auth_path());
    let configured = configured_providers(data_dir, codex_auth.as_ref());
    let mut models = cached_snapshot(data_dir, codex_auth.as_ref());
    let mut failed = Vec::new();

    if configured.iter().any(|provider| provider == "openai-codex") && codex_auth.is_none() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Ok(Some(credential)) = auth.resolve_oauth("openai-codex")
            && credential.expires <= now.saturating_add(60)
            && let Ok(refreshed) = model::refresh_oauth(&credential).await
        {
            let _ = auth.store_oauth("openai-codex", refreshed);
        }
    }

    let deepseek_future = async {
        if !configured.iter().any(|provider| provider == "deepseek") {
            return None;
        }
        let key = auth
            .resolve_api_key("deepseek", "DEEPSEEK_API_KEY")
            .ok()
            .flatten()?;
        Some(
            registry
                .discover(&DeepSeekProvider::new(DeepSeekConfig::from_api_key(
                    None, key,
                )))
                .await,
        )
    };
    let openai_future = async {
        if !configured.iter().any(|provider| provider == "openai") {
            return None;
        }
        let key = auth
            .resolve_api_key("openai", "OPENAI_API_KEY")
            .ok()
            .flatten()?;
        Some(
            registry
                .discover(&OpenAiProvider::new(OpenAiConfig::from_api_key(None, key)))
                .await,
        )
    };
    let codex_future = async {
        if !configured.iter().any(|provider| provider == "openai-codex") {
            return None;
        }
        let provider = codex_provider(&auth, codex_auth.clone(), None)?;
        Some(registry.discover(&provider).await)
    };
    let (deepseek, openai, codex) = tokio::join!(deepseek_future, openai_future, codex_future);

    let compatible_futures = configured
        .iter()
        .filter(|provider| !matches!(provider.as_str(), "deepseek" | "openai" | "openai-codex"))
        .filter_map(|provider_id| compatible_provider(&auth, provider_id))
        .map(|provider| async move {
            ModelRegistry::new(crate::ax_models_dir())
                .discover(&provider)
                .await
        });
    let compatible = futures_util::future::join_all(compatible_futures).await;

    if let Some(catalog) = deepseek {
        replace_provider(&mut models, &catalog.provider, catalog.models);
    } else if configured.iter().any(|provider| provider == "deepseek") {
        failed.push("deepseek".to_owned());
    }
    if let Some(catalog) = openai {
        replace_provider(&mut models, &catalog.provider, catalog.models);
    } else if configured.iter().any(|provider| provider == "openai") {
        failed.push("openai".to_owned());
    }
    if let Some(catalog) = codex {
        replace_provider(&mut models, &catalog.provider, catalog.models);
    } else if configured.iter().any(|provider| provider == "openai-codex") {
        failed.push("openai-codex".to_owned());
    }
    for catalog in compatible {
        if catalog.models.is_empty() {
            failed.push(catalog.provider);
        } else {
            replace_provider(&mut models, &catalog.provider, catalog.models);
        }
    }
    sort_models(&mut models);
    CatalogRefreshResult { models, failed }
}

fn compatible_provider(auth: &AuthStorage, provider_id: &str) -> Option<DeepSeekProvider> {
    let spec = model::provider(provider_id)?;
    if spec.protocol != ProviderProtocol::OpenAiCompatible {
        return None;
    }
    let environment = spec.environment?;
    let key = auth
        .resolve_api_key(provider_id, environment)
        .ok()
        .flatten()?;
    let endpoint = model::provider_chat_endpoint(provider_id)?;
    Some(DeepSeekProvider::new(DeepSeekConfig::from_compatible(
        provider_id,
        "catalog-only".to_owned(),
        key,
        endpoint,
        128_000,
    )))
}

fn sort_models(models: &mut Vec<ModelInfo>) {
    models.sort_by(|a, b| a.provider.cmp(&b.provider).then(a.id.cmp(&b.id)));
    models.dedup_by(|a, b| a.provider == b.provider && a.id == b.id);
}

fn replace_provider(models: &mut Vec<ModelInfo>, provider: &str, replacement: Vec<ModelInfo>) {
    models.retain(|model| model.provider != provider);
    models.extend(replacement);
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn explicit_codex_auth_is_available_without_ax_auth_entry() {
        let root = std::env::temp_dir().join(format!(
            "ax-catalog-auth-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let codex_auth = root.join("codex-auth.json");
        fs::write(&codex_auth, r#"{"tokens":{"access_token":"test-token"}}"#).unwrap();

        let configured = configured_providers(&root, Some(&codex_auth));
        assert!(configured.iter().any(|provider| provider == "openai-codex"));

        fs::remove_dir_all(root).unwrap();
    }
}
