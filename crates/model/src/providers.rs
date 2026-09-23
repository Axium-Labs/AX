//! Built-in provider metadata ported from pi's MIT-licensed provider catalog.
//!
//! This module deliberately describes authentication and wire protocol
//! separately. Sharing an API-key dialog does not imply that Anthropic,
//! Google, Bedrock, and `OpenAI` use the same request schema.

use crate::{ModelError, ModelInfo, OpenAiCompatibleConfig, ReasoningEffort};

pub const DEEPSEEK_FALLBACK_MODEL: &str = "deepseek-flash";
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const DEEPSEEK_CONTEXT_WINDOW: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderAuthKind {
    ApiKey,
    CodexOAuth,
    ExternalOAuth,
    Ambient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderProtocol {
    OpenAiCompatible,
    OpenAiResponses,
    Anthropic,
    Google,
    Bedrock,
    Managed,
}

#[derive(Clone, Copy, Debug)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub environment: Option<&'static str>,
    pub auth: ProviderAuthKind,
    pub protocol: ProviderProtocol,
}

macro_rules! key {
    ($id:literal, $name:literal, $env:literal, $protocol:ident) => {
        ProviderSpec {
            id: $id,
            name: $name,
            environment: Some($env),
            auth: ProviderAuthKind::ApiKey,
            protocol: ProviderProtocol::$protocol,
        }
    };
}

/// Provider inventory kept in the same order as pi's built-in registry.
pub static PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "amazon-bedrock",
        name: "Amazon Bedrock",
        environment: None,
        auth: ProviderAuthKind::Ambient,
        protocol: ProviderProtocol::Bedrock,
    },
    key!("ant-ling", "Ant Ling", "ANT_LING_API_KEY", OpenAiCompatible),
    key!("anthropic", "Anthropic", "ANTHROPIC_API_KEY", Anthropic),
    key!(
        "azure-openai-responses",
        "Azure OpenAI",
        "AZURE_OPENAI_API_KEY",
        OpenAiResponses
    ),
    key!("baseten", "Baseten", "BASETEN_API_KEY", OpenAiCompatible),
    key!("cerebras", "Cerebras", "CEREBRAS_API_KEY", OpenAiCompatible),
    key!(
        "cloudflare-ai-gateway",
        "Cloudflare AI Gateway",
        "CLOUDFLARE_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "cloudflare-workers-ai",
        "Cloudflare Workers AI",
        "CLOUDFLARE_API_KEY",
        OpenAiCompatible
    ),
    key!("deepseek", "DeepSeek", "DEEPSEEK_API_KEY", OpenAiCompatible),
    key!(
        "fireworks",
        "Fireworks",
        "FIREWORKS_API_KEY",
        OpenAiCompatible
    ),
    ProviderSpec {
        id: "github-copilot",
        name: "GitHub Copilot",
        environment: Some("COPILOT_GITHUB_TOKEN"),
        auth: ProviderAuthKind::ExternalOAuth,
        protocol: ProviderProtocol::Managed,
    },
    key!("google", "Google Gemini", "GEMINI_API_KEY", Google),
    ProviderSpec {
        id: "google-vertex",
        name: "Google Vertex",
        environment: Some("GOOGLE_CLOUD_API_KEY"),
        auth: ProviderAuthKind::Ambient,
        protocol: ProviderProtocol::Google,
    },
    key!("groq", "Groq", "GROQ_API_KEY", OpenAiCompatible),
    key!("huggingface", "Hugging Face", "HF_TOKEN", OpenAiCompatible),
    key!(
        "kimi-coding",
        "Kimi Coding",
        "KIMI_API_KEY",
        OpenAiCompatible
    ),
    key!("meta", "Meta", "META_API_KEY", OpenAiCompatible),
    key!("minimax", "MiniMax", "MINIMAX_API_KEY", OpenAiCompatible),
    key!(
        "minimax-cn",
        "MiniMax CN",
        "MINIMAX_CN_API_KEY",
        OpenAiCompatible
    ),
    key!("mistral", "Mistral", "MISTRAL_API_KEY", OpenAiCompatible),
    key!(
        "moonshotai",
        "Moonshot AI",
        "MOONSHOT_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "moonshotai-cn",
        "Moonshot AI CN",
        "MOONSHOT_API_KEY",
        OpenAiCompatible
    ),
    key!("nvidia", "NVIDIA", "NVIDIA_API_KEY", OpenAiCompatible),
    key!("openai", "OpenAI", "OPENAI_API_KEY", OpenAiResponses),
    ProviderSpec {
        id: "openai-codex",
        name: "OpenAI Codex",
        environment: None,
        auth: ProviderAuthKind::CodexOAuth,
        protocol: ProviderProtocol::OpenAiResponses,
    },
    key!("opencode", "OpenCode", "OPENCODE_API_KEY", OpenAiCompatible),
    key!(
        "opencode-go",
        "OpenCode Go",
        "OPENCODE_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "openrouter",
        "OpenRouter",
        "OPENROUTER_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "qwen-token-plan",
        "Qwen Token Plan",
        "QWEN_TOKEN_PLAN_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "qwen-token-plan-cn",
        "Qwen Token Plan CN",
        "QWEN_TOKEN_PLAN_CN_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "qwen-token-plan-individual",
        "Qwen Individual",
        "QWEN_TOKEN_PLAN_API_KEY",
        OpenAiCompatible
    ),
    key!("radius", "Radius", "RADIUS_API_KEY", OpenAiCompatible),
    key!("together", "Together", "TOGETHER_API_KEY", OpenAiCompatible),
    key!(
        "vercel-ai-gateway",
        "Vercel AI Gateway",
        "AI_GATEWAY_API_KEY",
        OpenAiCompatible
    ),
    key!("xai", "xAI", "XAI_API_KEY", OpenAiCompatible),
    key!("xiaomi", "Xiaomi", "XIAOMI_API_KEY", OpenAiCompatible),
    key!(
        "xiaomi-token-plan-ams",
        "Xiaomi Token Plan AMS",
        "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "xiaomi-token-plan-cn",
        "Xiaomi Token Plan CN",
        "XIAOMI_TOKEN_PLAN_CN_API_KEY",
        OpenAiCompatible
    ),
    key!(
        "xiaomi-token-plan-sgp",
        "Xiaomi Token Plan SGP",
        "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
        OpenAiCompatible
    ),
    key!("zai", "Z.AI", "ZAI_API_KEY", OpenAiCompatible),
    key!(
        "zai-coding-cn",
        "Z.AI Coding CN",
        "ZAI_CODING_CN_API_KEY",
        OpenAiCompatible
    ),
];

#[must_use]
pub fn provider(id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

/// Pi provider base URLs for adapters whose public API follows the `OpenAI`
/// compatibility shape. AX uses these only to perform authenticated live
/// discovery; model ids still come from each provider's `/models` response.
#[must_use]
pub fn provider_base_url(id: &str) -> Option<&'static str> {
    Some(match id {
        "ant-ling" => "https://api.ant-ling.com/v1",
        "baseten" => "https://inference.baseten.co/v1",
        "cerebras" => "https://api.cerebras.ai/v1",
        "deepseek" => DEEPSEEK_BASE_URL,
        "fireworks" => "https://api.fireworks.ai/inference",
        "groq" => "https://api.groq.com/openai/v1",
        "huggingface" => "https://router.huggingface.co/v1",
        "kimi-coding" => "https://api.kimi.com/coding",
        "meta" => "https://api.meta.ai/v1",
        "mistral" => "https://api.mistral.ai/v1",
        "moonshotai" => "https://api.moonshot.ai/v1",
        "moonshotai-cn" => "https://api.moonshot.cn/v1",
        "nvidia" => "https://integrate.api.nvidia.com/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "qwen-token-plan" | "qwen-token-plan-individual" => {
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"
        }
        "qwen-token-plan-cn" => {
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
        }
        "together" => "https://api.together.ai/v1",
        "xai" => "https://api.x.ai/v1",
        "xiaomi" => "https://api.xiaomimimo.com/v1",
        "xiaomi-token-plan-ams" => "https://token-plan-ams.xiaomimimo.com/v1",
        "xiaomi-token-plan-cn" => "https://token-plan-cn.xiaomimimo.com/v1",
        "xiaomi-token-plan-sgp" => "https://token-plan-sgp.xiaomimimo.com/v1",
        "zai" => "https://api.z.ai/api/coding/paas/v4",
        "zai-coding-cn" => "https://open.bigmodel.cn/api/coding/paas/v4",
        _ => return None,
    })
}

#[must_use]
pub fn provider_chat_endpoint(id: &str) -> Option<String> {
    provider_base_url(id).map(|base| format!("{}/chat/completions", base.trim_end_matches('/')))
}

/// `DeepSeek`'s provider defaults, kept outside the shared wire adapter.
#[must_use]
pub fn deepseek_compatible_config(
    model: Option<String>,
    api_key: String,
) -> OpenAiCompatibleConfig {
    let endpoint = std::env::var("DEEPSEEK_API_URL")
        .unwrap_or_else(|_| format!("{DEEPSEEK_BASE_URL}/chat/completions"));
    OpenAiCompatibleConfig::new(
        "deepseek",
        model.unwrap_or_else(|| DEEPSEEK_FALLBACK_MODEL.to_owned()),
        api_key,
        endpoint,
        DEEPSEEK_CONTEXT_WINDOW,
    )
}

// Keep the old public constructors callable through the deprecated type alias.
impl OpenAiCompatibleConfig {
    #[deprecated(
        note = "use deepseek_compatible_config for DeepSeek or OpenAiCompatibleConfig::new for other providers"
    )]
    #[must_use]
    pub fn from_api_key(model: Option<String>, api_key: String) -> Self {
        deepseek_compatible_config(model, api_key)
    }

    /// # Errors
    ///
    /// Returns [`ModelError::Configuration`] when `DEEPSEEK_API_KEY` is absent.
    #[deprecated(
        note = "resolve DeepSeek credentials in the caller, then use deepseek_compatible_config"
    )]
    pub fn from_env(model: Option<String>) -> Result<Self, ModelError> {
        let api_key = std::env::var("DEEPSEEK_API_KEY")
            .map_err(|_| ModelError::Configuration("DEEPSEEK_API_KEY is not set".to_owned()))?;
        Ok(deepseek_compatible_config(model, api_key))
    }

    #[deprecated(note = "use OpenAiCompatibleConfig::new")]
    #[must_use]
    pub fn from_compatible(
        provider_id: impl Into<String>,
        model: String,
        api_key: String,
        endpoint: String,
        context_window: usize,
    ) -> Self {
        Self::new(provider_id, model, api_key, endpoint, context_window)
    }
}

pub(crate) fn compatible_model_info(id: String, provider: &str, endpoint: &str) -> ModelInfo {
    let deepseek = provider == "deepseek";
    let reasoning = deepseek && id.to_ascii_lowercase().contains("reason");
    ModelInfo {
        display_name: id.clone(),
        id,
        provider: provider.to_owned(),
        context_window: if deepseek {
            DEEPSEEK_CONTEXT_WINDOW
        } else {
            128_000
        },
        max_output_tokens: None,
        reasoning_efforts: if reasoning {
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]
        } else {
            Vec::new()
        },
        default_reasoning_effort: reasoning.then_some(ReasoningEffort::High),
        supports_tools: true,
        endpoint: (!deepseek).then(|| endpoint.to_owned()),
    }
}

/// Providers for which pi exposes an account/subscription OAuth login in
/// addition to (or instead of) an API-key login.
#[must_use]
pub fn provider_supports_oauth(id: &str) -> bool {
    matches!(
        id,
        "anthropic"
            | "github-copilot"
            | "kimi-coding"
            | "meta"
            | "openai-codex"
            | "openrouter"
            | "radius"
            | "xai"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelProvider, OpenAiCompatibleProvider};

    #[test]
    fn compatible_adapter_uses_selected_vendor_identity() {
        for id in [
            "deepseek",
            "groq",
            "mistral",
            "openrouter",
            "together",
            "xai",
            "kimi-coding",
        ] {
            assert_eq!(
                provider(id).unwrap().protocol,
                ProviderProtocol::OpenAiCompatible
            );
            let config = OpenAiCompatibleConfig::new(
                id,
                "test-model".to_owned(),
                "test-key".to_owned(),
                provider_chat_endpoint(id).unwrap(),
                128_000,
            );
            let adapter = OpenAiCompatibleProvider::new(config);
            assert_eq!(adapter.name(), id);
            assert_eq!(adapter.model_id(), "test-model");
        }
    }

    #[test]
    fn deepseek_defaults_and_discovery_metadata_stay_provider_specific() {
        let config = deepseek_compatible_config(None, "test-key".to_owned());
        assert_eq!(config.provider_id, "deepseek");
        assert_eq!(config.model, DEEPSEEK_FALLBACK_MODEL);
        assert_eq!(config.context_window, DEEPSEEK_CONTEXT_WINDOW);

        let deepseek = compatible_model_info(
            "deepseek-reasoner".to_owned(),
            "deepseek",
            "https://api.deepseek.com/chat/completions",
        );
        let groq = compatible_model_info(
            "groq-model".to_owned(),
            "groq",
            "https://api.groq.com/openai/v1/chat/completions",
        );
        assert_eq!(deepseek.context_window, DEEPSEEK_CONTEXT_WINDOW);
        assert_eq!(
            deepseek.default_reasoning_effort,
            Some(ReasoningEffort::High)
        );
        assert!(deepseek.endpoint.is_none());
        assert_eq!(groq.context_window, 128_000);
        assert!(groq.reasoning_efforts.is_empty());
        assert_eq!(
            groq.endpoint.as_deref(),
            Some("https://api.groq.com/openai/v1/chat/completions")
        );
    }

    #[test]
    #[allow(deprecated)]
    fn legacy_public_aliases_still_construct_the_adapter() {
        let config = crate::DeepSeekConfig::from_api_key(None, "test-key".to_owned());
        let adapter = crate::DeepSeekProvider::new(config);
        assert_eq!(adapter.name(), "deepseek");

        let compatible = crate::DeepSeekConfig::from_compatible(
            "groq",
            "test-model".to_owned(),
            "test-key".to_owned(),
            "https://api.groq.com/openai/v1/chat/completions".to_owned(),
            128_000,
        );
        assert_eq!(crate::DeepSeekProvider::new(compatible).name(), "groq");
    }
}
