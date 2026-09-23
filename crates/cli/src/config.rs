//! User-level AX configuration, persisted at `~/.ax/config.json`.
//!
//! AX keeps runtime state and credentials separate: the session database
//! lives in the project's `.ax` directory, credentials in `~/.ax/auth.json`,
//! and the last successfully selected model here. CLI resolution prefers
//! explicit flags over this file, then falls back to local provider
//! detection (see `model_selection`).

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// `AX_HOME` when set, otherwise `~/.ax`, mirroring the credential store.
#[must_use]
pub(crate) fn ax_home() -> PathBuf {
    if let Some(root) = std::env::var_os("AX_HOME") {
        return PathBuf::from(root);
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".ax")
}

#[must_use]
pub(crate) fn config_path() -> PathBuf {
    ax_home().join("config.json")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AxConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelConfig>,
}

/// The last model selection AX successfully switched to via `/model`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

impl AxConfig {
    /// Loads the user-level config; a missing file is treated as empty.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be parsed.
    pub fn load() -> Result<Self> {
        Self::load_from_home(&ax_home())
    }

    pub(crate) fn load_from_home(home: &Path) -> Result<Self> {
        let path = home.join("config.json");
        if path.is_file() {
            return Self::load_from(&path);
        }
        let legacy = home.join("config.toml");
        match fs::read_to_string(&legacy) {
            Ok(contents) => {
                let config: Self =
                    toml::from_str(&contents).context("invalid legacy config.toml")?;
                config.save_to(&path)?;
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).context("failed to read legacy config.toml"),
        }
    }

    /// Loads a config from a specific path (tests inject a temporary file).
    pub(crate) fn load_from(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).context("invalid config.json"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).context("failed to read config.json"),
        }
    }

    /// Persists the config atomically (write temp file, then rename).
    ///
    /// # Errors
    ///
    /// Returns an error when the config cannot be encoded or written.
    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    /// Persists the config to a specific path (tests inject a temporary file).
    pub(crate) fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents =
            serde_json::to_string_pretty(self).context("failed to encode config.json")?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, contents)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        fs::rename(&temporary, path).with_context(|| {
            format!(
                "failed to move {} to {}",
                temporary.display(),
                path.display()
            )
        })?;
        Ok(())
    }

    /// The persisted model selection, if any.
    #[must_use]
    pub fn model_config(&self) -> Option<&ModelConfig> {
        self.model.as_ref()
    }
}
