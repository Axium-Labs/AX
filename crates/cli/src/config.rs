//! AX configuration, persisted in `.ax/config.json` beside the executable.
//!
//! Sessions, memory and credentials share the installation-owned AX home.
//! `AX_HOME` explicitly overrides that location. CLI resolution prefers
//! explicit flags over this file, then falls back to local provider
//! detection (see `model_selection`).

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// `AX_HOME` when set, otherwise `.ax` beside the running executable.
#[must_use]
pub(crate) fn ax_home() -> PathBuf {
    if let Some(root) = std::env::var_os("AX_HOME") {
        return PathBuf::from(root);
    }
    std::env::current_exe()
        .expect("cannot locate the AX executable")
        .parent()
        .expect("AX executable has no installation directory")
        .join(".ax")
}

#[cfg(test)]
mod subagent_tests {
    use super::*;

    #[test]
    fn subagent_defaults_and_json_persistence() {
        let root =
            std::env::temp_dir().join(format!("ax-subagent-config-{}", uuid::Uuid::new_v4()));
        let path = root.join("config.json");
        let mut config = AxConfig::load_from(&path).unwrap();
        assert_eq!(config.subagent, runtime_core::SubagentConfig::default());
        assert!(
            !serde_json::from_str::<AxConfig>("{}")
                .unwrap()
                .subagent
                .enabled
        );
        config.subagent.enabled = true;
        config.subagent.max_concurrent = 2;
        config.save_to(&path).unwrap();
        assert_eq!(
            AxConfig::load_from(&path).unwrap().subagent,
            config.subagent
        );
        config.subagent.enabled = false;
        config.save_to(&path).unwrap();
        assert!(!AxConfig::load_from(&path).unwrap().subagent.enabled);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_subagent_section_migrates_without_losing_settings() {
        let root = std::env::temp_dir().join(format!("ax-subagent-toml-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("config.toml"),
            "[subagent]\nenabled = true\nmax_concurrent = 3\nmax_depth = 1\n",
        )
        .unwrap();
        let config = AxConfig::load_from_home(&root).unwrap();
        assert!(config.subagent.enabled);
        assert_eq!(config.subagent.max_concurrent, 3);
        assert_eq!(config.subagent.max_depth, 1);
        assert_eq!(
            AxConfig::load_from_home(&root).unwrap().subagent,
            config.subagent
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[must_use]
pub(crate) fn config_path() -> PathBuf {
    ax_home().join("config.json")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AxConfig {
    #[serde(default)]
    pub verification: runtime_core::VerificationConfig,
    #[serde(default)]
    pub child_models: std::collections::BTreeMap<String, ModelConfig>,
    #[serde(default)]
    pub context_pool: runtime_core::ContextPoolPolicy,
    #[serde(default)]
    pub permissions: tool::PermissionProfile,
    #[serde(default)]
    pub retry: model::RetryPolicy,
    #[serde(default)]
    pub sandbox: sandbox::SandboxMode,
    #[serde(default)]
    pub subagent: runtime_core::SubagentConfig,
    #[serde(default)]
    pub execution: ExecutionConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inference: Option<InferenceConfig>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ExecutionConfig {
    #[serde(default)]
    pub environment: AgentEnvironment,
    #[serde(default)]
    pub terminal_shell: TerminalShell,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum AgentEnvironment {
    #[default]
    Native,
    Wsl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum TerminalShell {
    #[default]
    Powershell,
    Cmd,
    GitBash,
    Wsl,
}

/// The last model selection AX successfully switched to via `/model`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

/// Inference strategy. Only two modes exist, deliberately: `Standard` sends a
/// single request; `Fast` races a hedged secondary against a slow primary.
/// There is no Race/Aggressive tier on top of this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InferenceMode {
    /// One request per model call — the historical behavior.
    #[default]
    Standard,
    /// Adaptive hedging: a secondary request fires when the primary exceeds
    /// the learned TTFT threshold. Latency only; model, reasoning effort and
    /// output limits are never altered.
    Fast,
}

/// Inference settings. Missing entirely means Standard mode.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InferenceConfig {
    #[serde(default)]
    pub mode: InferenceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<FastConfig>,
}

/// Advanced overrides for Fast mode. Ordinary users should leave both unset:
/// the hedge threshold is learned from the provider's historical TTFT.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FastConfig {
    /// Fixed hedge delay in milliseconds. `None` = adaptive (P95 of the
    /// primary provider's recent TTFT, clamped to a sane range).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hedge_threshold_ms: Option<u64>,
    /// Maximum concurrent inference requests; default 2 (primary + one
    /// secondary). 1 effectively disables hedging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
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
