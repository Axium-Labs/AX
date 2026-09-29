use std::{fs, path::PathBuf, time::SystemTime};

use serde::{Deserialize, Serialize};

use crate::{ModelError, ModelInfo, ModelProvider};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogSource {
    Live,
    Cache,
    Fallback,
}

#[derive(Clone, Debug)]
pub struct ModelCatalog {
    pub provider: String,
    pub models: Vec<ModelInfo>,
    pub source: CatalogSource,
    pub warning: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    saved_at_unix: u64,
    models: Vec<ModelInfo>,
}

pub struct ModelRegistry {
    cache_dir: PathBuf,
}

impl ModelRegistry {
    #[must_use]
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
        }
    }

    pub async fn discover(&self, provider: &dyn ModelProvider) -> ModelCatalog {
        match provider.list_models().await {
            Ok(models) if !models.is_empty() => {
                let warning = self
                    .save(provider.name(), &models)
                    .err()
                    .map(|error| error.to_string());
                ModelCatalog {
                    provider: provider.name().to_owned(),
                    models,
                    source: CatalogSource::Live,
                    warning,
                }
            }
            Ok(_) => self.cached_or_fallback(provider, "provider returned an empty catalog"),
            Err(error) => self.cached_or_fallback(provider, &error.to_string()),
        }
    }

    /// Loads an AX catalog cache without attempting a network request, then
    /// falls back to the provider's minimal bootstrap catalog.
    #[must_use]
    pub fn cached(&self, provider: &dyn ModelProvider, reason: &str) -> ModelCatalog {
        self.cached_or_fallback(provider, reason)
    }

    fn cached_or_fallback(&self, provider: &dyn ModelProvider, warning: &str) -> ModelCatalog {
        if let Ok(models) = self.load(provider.name())
            && !models.is_empty()
        {
            return ModelCatalog {
                provider: provider.name().to_owned(),
                models,
                source: CatalogSource::Cache,
                warning: Some(warning.to_owned()),
            };
        }
        ModelCatalog {
            provider: provider.name().to_owned(),
            models: self.bootstrap_models(provider),
            source: CatalogSource::Fallback,
            warning: Some(warning.to_owned()),
        }
    }

    fn bootstrap_models(&self, provider: &dyn ModelProvider) -> Vec<ModelInfo> {
        let local = fs::read(self.cache_dir.join("pi-catalog.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<ModelInfo>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|item| item.provider == provider.name() && item.supports_tools)
            .collect::<Vec<_>>();
        if local.is_empty() {
            provider.fallback_models()
        } else {
            local
        }
    }

    fn cache_path(&self, provider: &str) -> PathBuf {
        self.cache_dir.join(format!("{provider}.json"))
    }

    fn save(&self, provider: &str, models: &[ModelInfo]) -> Result<(), ModelError> {
        fs::create_dir_all(&self.cache_dir)?;
        let cache = CacheFile {
            saved_at_unix: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            models: models.to_vec(),
        };
        let path = self.cache_path(provider);
        let temporary = path.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_vec_pretty(&cache).map_err(|error| {
                ModelError::InvalidResponse(format!("failed to encode model cache: {error}"))
            })?,
        )?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    fn load(&self, provider: &str) -> Result<Vec<ModelInfo>, ModelError> {
        let contents = fs::read(self.cache_path(provider))?;
        serde_json::from_slice::<CacheFile>(&contents)
            .map(|cache| cache.models)
            .map_err(|error| ModelError::InvalidResponse(format!("invalid model cache: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelRequest, ModelResponse};
    struct RejectedProvider;
    #[async_trait::async_trait]
    impl ModelProvider for RejectedProvider {
        fn name(&self) -> &'static str {
            "minimax"
        }
        fn model_id(&self) -> &'static str {
            "catalog-only"
        }
        fn context_window(&self) -> usize {
            128_000
        }
        async fn complete(&self, _: ModelRequest) -> Result<ModelResponse, ModelError> {
            unreachable!()
        }
        async fn list_models(&self) -> Result<Vec<ModelInfo>, ModelError> {
            Err(ModelError::HttpStatus {
                status: 401,
                message: "invalid key".into(),
            })
        }
        fn fallback_models(&self) -> Vec<ModelInfo> {
            crate::builtin_models("minimax")
        }
    }
    #[tokio::test]
    async fn failed_discovery_retains_error_and_offline_models() {
        let registry = ModelRegistry::new(
            std::env::temp_dir().join(format!("ax-discovery-test-{}", std::process::id())),
        );
        let result = registry.discover(&RejectedProvider).await;
        assert_eq!(result.source, CatalogSource::Fallback);
        assert!(!result.models.is_empty());
        assert!(result.warning.unwrap().contains("401"));
    }
}
