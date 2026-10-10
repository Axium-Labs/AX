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
    PiMessages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchCapability {
    /// No native web search capability
    None,
    /// Provider supports native web search (e.g., via browsing tool)
    Native,
}

#[derive(Clone, Copy, Debug)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub environment: Option<&'static str>,
    pub auth: ProviderAuthKind,
    pub protocol: ProviderProtocol,
    pub search_capability: SearchCapability,
}

macro_rules! key {
    ($id:literal, $name:literal, $env:literal, $protocol:ident) => {
        ProviderSpec {
            id: $id,
            name: $name,
            environment: Some($env),
            auth: ProviderAuthKind::ApiKey,
            protocol: ProviderProtocol::$protocol,
            search_capability: SearchCapability::None,
        }
    };
}

macro_rules! key_with_search {
    ($id:literal, $name:literal, $env:literal, $protocol:ident, $search:ident) => {
        ProviderSpec {
            id: $id,
            name: $name,
            environment: Some($env),
            auth: ProviderAuthKind::ApiKey,
            protocol: ProviderProtocol::$protocol,
            search_capability: SearchCapability::$search,
        }
    };
}

/// Provider inventory kept in the same order as pi's built-in registry.
pub static PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "workbuddy-cn",
        name: "WorkBuddy China",
        environment: None,
        auth: ProviderAuthKind::ExternalOAuth,
        protocol: ProviderProtocol::Managed,
        search_capability: SearchCapability::None,
    },
    ProviderSpec {
        id: "workbuddy",
        name: "WorkBuddy International",
        environment: None,
        auth: ProviderAuthKind::ExternalOAuth,
        protocol: ProviderProtocol::Managed,
        search_capability: SearchCapability::None,
    },
    ProviderSpec {
        id: "amazon-bedrock",
        name: "Amazon Bedrock",
        environment: Some("AWS_BEARER_TOKEN_BEDROCK"),
        auth: ProviderAuthKind::Ambient,
        protocol: ProviderProtocol::Bedrock,
        search_capability: SearchCapability::None,
    },
    key!("ant-ling", "Ant Ling", "ANT_LING_API_KEY", OpenAiCompatible),
    key_with_search!("anthropic", "Anthropic", "ANTHROPIC_API_KEY", Anthropic, Native),
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
        search_capability: SearchCapability::None,
    },
    key_with_search!("google", "Google Gemini", "GEMINI_API_KEY", Google, Native),
    ProviderSpec {
        id: "google-vertex",
        name: "Google Vertex",
        environment: Some("GOOGLE_CLOUD_API_KEY"),
        auth: ProviderAuthKind::Ambient,
        protocol: ProviderProtocol::Google,
        search_capability: SearchCapability::Native,
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
    key_with_search!("openai", "OpenAI", "OPENAI_API_KEY", OpenAiResponses, Native),
    ProviderSpec {
        id: "openai-codex",
        name: "OpenAI Codex",
        environment: None,
        auth: ProviderAuthKind::CodexOAuth,
        protocol: ProviderProtocol::OpenAiResponses,
        search_capability: SearchCapability::Native,
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
    key!("radius", "Radius", "RADIUS_API_KEY", PiMessages),
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

/// OpenAI-compatible base URLs. Account placeholders are expanded locally.
/// Missing endpoint configuration does not mean the runtime adapter is absent.
#[must_use]
pub fn provider_base_url(id: &str) -> Option<String> {
    let template = match id {
        "ant-ling" => "https://api.ant-ling.com/v1",
        "baseten" => "https://inference.baseten.co/v1",
        "cerebras" => "https://api.cerebras.ai/v1",
        "cloudflare-ai-gateway" => {
            "https://gateway.ai.cloudflare.com/v1/${CLOUDFLARE_ACCOUNT_ID}/${CLOUDFLARE_GATEWAY_ID}/compat"
        }
        "cloudflare-workers-ai" => {
            "https://api.cloudflare.com/client/v4/accounts/${CLOUDFLARE_ACCOUNT_ID}/ai/v1"
        }
        "deepseek" => DEEPSEEK_BASE_URL,
        "fireworks" => "https://api.fireworks.ai/inference/v1",
        "groq" => "https://api.groq.com/openai/v1",
        "huggingface" => "https://router.huggingface.co/v1",
        "kimi-coding" => "https://api.kimi.com/coding",
        "meta" => "https://api.meta.ai/v1",
        "minimax" => "https://api.minimax.io/v1",
        "minimax-cn" => "https://api.minimax.cn/v1",
        "mistral" => "https://api.mistral.ai/v1",
        "moonshotai" => "https://api.moonshot.ai/v1",
        "moonshotai-cn" => "https://api.moonshot.cn/v1",
        "nvidia" => "https://integrate.api.nvidia.com/v1",
        "opencode" => "https://opencode.ai/zen/v1",
        "opencode-go" => "https://opencode.ai/zen/go/v1",
        "openrouter" => "https://openrouter.ai/api/v1",
        "qwen-token-plan" | "qwen-token-plan-individual" => {
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"
        }
        "qwen-token-plan-cn" => {
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
        }
        "together" => "https://api.together.ai/v1",
        "vercel-ai-gateway" => "https://ai-gateway.vercel.sh/v1",
        "xai" => "https://api.x.ai/v1",
        "xiaomi" => "https://api.xiaomimimo.com/v1",
        "xiaomi-token-plan-ams" => "https://token-plan-ams.xiaomimimo.com/v1",
        "xiaomi-token-plan-cn" => "https://token-plan-cn.xiaomimimo.com/v1",
        "xiaomi-token-plan-sgp" => "https://token-plan-sgp.xiaomimimo.com/v1",
        "zai" => "https://api.z.ai/api/coding/paas/v4",
        "zai-coding-cn" => "https://open.bigmodel.cn/api/coding/paas/v4",
        _ => return None,
    };
    expand_environment(template)
}

/// Substitutes every `${VAR}` in an endpoint template. Returns `None` when a
/// referenced variable is unset or empty, so an account-scoped provider reads
/// as missing configuration instead of yielding a broken URL.
fn expand_environment(template: &str) -> Option<String> {
    let mut expanded = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}')?;
        let name = &after[..end];
        let value = std::env::var(name).ok().filter(|value| !value.is_empty())?;
        expanded.push_str(&value);
        rest = &after[end + 1..];
    }
    expanded.push_str(rest);
    Some(expanded)
}

/// The one definition of "AX can drive this provider".
///
/// A provider qualifies when its wire protocol has a runtime adapter.
/// Missing account/resource fields are reported separately as configuration errors. The CLI filter and the Crew status surface
/// both read this, so a catalog entry can never look usable in one place and
/// unsupported in another.
#[must_use]
pub fn provider_supported(id: &str) -> bool {
    if crate::workbuddy::WorkBuddyRegion::from_provider_id(id).is_some() {
        return true;
    }
    provider(id).is_some_and(|spec| {
        matches!(
            spec.protocol,
            ProviderProtocol::OpenAiCompatible
                | ProviderProtocol::OpenAiResponses
                | ProviderProtocol::Anthropic
                | ProviderProtocol::Google
                | ProviderProtocol::Bedrock
                | ProviderProtocol::PiMessages
        )
    })
}

/// Why a catalog provider cannot be driven, for surfaces that must not offer
/// a credential dialog which would silently do nothing. `None` means AX can
/// drive it.
#[must_use]
pub fn provider_unsupported_reason(id: &str) -> Option<&'static str> {
    if provider_supported(id) {
        return None;
    }
    Some(match id {
        "github-copilot" => "hosted OAuth only, with no runtime adapter in AX",
        _ => "unknown provider or no runtime adapter in AX",
    })
}

#[must_use]
pub fn provider_chat_endpoint(id: &str) -> Option<String> {
    provider_base_url(id).map(|base| format!("{}/chat/completions", base.trim_end_matches('/')))
}

/// Missing non-secret endpoint fields, separate from adapter support.
#[must_use]
pub fn provider_configuration_reason(id: &str) -> Option<&'static str> {
    let present = |name| std::env::var(name).is_ok_and(|v| !v.trim().is_empty());
    match id {
        "cloudflare-workers-ai" if !present("CLOUDFLARE_ACCOUNT_ID") => {
            Some("CLOUDFLARE_ACCOUNT_ID is not set")
        }
        "cloudflare-ai-gateway"
            if !present("CLOUDFLARE_ACCOUNT_ID") || !present("CLOUDFLARE_GATEWAY_ID") =>
        {
            Some("Cloudflare Gateway requires CLOUDFLARE_ACCOUNT_ID and CLOUDFLARE_GATEWAY_ID")
        }
        "azure-openai-responses"
            if !present("AZURE_OPENAI_BASE_URL") && !present("AZURE_OPENAI_RESOURCE_NAME") =>
        {
            Some("Azure requires AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME")
        }
        _ => None,
    }
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

/// Model ids that name something other than a chat completion. Discovery gets
/// whatever a vendor lists, and providers ship ASR, TTS, embedding and reranker
/// entries beside their chat models. Marking those as tool-capable would offer
/// an agent a model that cannot call a tool.
///
/// The markers are the ones the bundled catalog itself uses: every id
/// containing one of them is `supports_tools: false` there, and no
/// tool-capable catalog id contains any of them.
fn supports_tool_calls(id: &str) -> bool {
    const NON_CHAT: &[&str] = &[
        "tts",
        "asr",
        "speech",
        "image-generation",
        "whisper",
        "embed",
        "rerank",
        "voiceclone",
        "voicedesign",
        "video",
    ];
    let lower = id.to_ascii_lowercase();
    !NON_CHAT.iter().any(|marker| lower.contains(marker))
}

pub(crate) fn compatible_model_info(id: String, provider: &str, endpoint: &str) -> ModelInfo {
    if let Some(mut known) = builtin_models(provider)
        .into_iter()
        .find(|item| item.id == id)
    {
        known.endpoint = (provider != "deepseek").then(|| endpoint.to_owned());
        return known;
    }
    let deepseek = provider == "deepseek";
    let reasoning = deepseek && id.to_ascii_lowercase().contains("reason");
    let supports_tools = supports_tool_calls(&id);
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
        supports_tools,
        endpoint: (!deepseek).then(|| endpoint.to_owned()),
    }
}

/// Account login flows implemented by AX (not merely advertised by pi).
#[must_use]
pub fn provider_supports_oauth(id: &str) -> bool {
    matches!(id, "openai-codex" | "workbuddy" | "workbuddy-cn")
}

/// Offline bootstrap catalog shipped with AX. These entries are not proof of access.
#[must_use]
pub fn builtin_models(provider_id: &str) -> Vec<ModelInfo> {
    static CATALOG: std::sync::LazyLock<Vec<ModelInfo>> = std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("catalog.json")).expect("valid bundled model catalog")
    });
    CATALOG
        .iter()
        .filter(|item| item.provider == provider_id && item.supports_tools)
        .cloned()
        .map(|mut item| {
            item.endpoint = provider_chat_endpoint(provider_id);
            item
        })
        .collect()
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

    /// Regression: Fireworks serves its OpenAI-compatible surface under
    /// `/inference/v1`. Without the `/v1` segment both paths AX derives from
    /// the base URL were dead: discovery hit `/inference/models` (404) and
    /// chat hit `/inference/chat/completions` instead of the documented
    /// `https://api.fireworks.ai/inference/v1/chat/completions`.
    #[test]
    fn fireworks_endpoints_keep_the_v1_segment() {
        assert_eq!(
            provider_base_url("fireworks").as_deref(),
            Some("https://api.fireworks.ai/inference/v1")
        );
        assert_eq!(
            provider_chat_endpoint("fireworks").as_deref(),
            Some("https://api.fireworks.ai/inference/v1/chat/completions")
        );
    }

    /// The catalog lists these providers and their models but `provider_base_url`
    /// used to return `None` for all of them, so `is_supported_provider` dropped
    /// them and a saved key silently did nothing. Each URL below is the
    /// catalog's own endpoint for that provider, verified against the live host.
    #[test]
    fn catalog_providers_with_known_endpoints_are_drivable() {
        for (id, base) in [
            ("minimax", "https://api.minimax.io/v1"),
            ("minimax-cn", "https://api.minimax.cn/v1"),
            ("opencode", "https://opencode.ai/zen/v1"),
            ("opencode-go", "https://opencode.ai/zen/go/v1"),
            ("vercel-ai-gateway", "https://ai-gateway.vercel.sh/v1"),
        ] {
            assert_eq!(provider_base_url(id).as_deref(), Some(base), "{id}");
            assert!(provider_supported(id), "{id} should be drivable");
            assert_eq!(provider_unsupported_reason(id), None, "{id}");
            assert_eq!(
                provider_chat_endpoint(id).unwrap(),
                format!("{base}/chat/completions"),
                "{id}"
            );
        }
    }

    /// Xiaomi's token-plan regions share model ids with the plain entry, so a
    /// selection that names a region must reach that region's host — the plain
    /// `api.xiaomimimo.com` host cannot serve a token-plan key.
    #[test]
    fn xiaomi_token_plan_regions_have_their_own_endpoints() {
        for (id, host) in [
            (
                "xiaomi-token-plan-cn",
                "https://token-plan-cn.xiaomimimo.com",
            ),
            (
                "xiaomi-token-plan-sgp",
                "https://token-plan-sgp.xiaomimimo.com",
            ),
            (
                "xiaomi-token-plan-ams",
                "https://token-plan-ams.xiaomimimo.com",
            ),
        ] {
            assert_eq!(
                provider_chat_endpoint(id).as_deref(),
                Some(format!("{host}/v1/chat/completions").as_str()),
                "{id}"
            );
            assert!(provider_supported(id), "{id}");
            assert!(
                !provider_chat_endpoint("xiaomi")
                    .unwrap()
                    .contains(host.trim_start_matches("https://"))
            );
        }
    }

    /// Missing account fields prevent URL construction, but do not hide an
    /// implemented adapter from credential setup.
    #[test]
    fn account_scoped_endpoints_require_their_variable() {
        if std::env::var("CLOUDFLARE_ACCOUNT_ID").is_ok_and(|value| !value.is_empty()) {
            return;
        }
        assert_eq!(provider_base_url("cloudflare-workers-ai"), None);
        assert!(provider_supported("cloudflare-workers-ai"));
        assert_eq!(
            provider_configuration_reason("cloudflare-workers-ai"),
            Some("CLOUDFLARE_ACCOUNT_ID is not set")
        );
    }

    #[test]
    fn environment_placeholders_expand_or_fail_closed() {
        let expanded = expand_environment("https://x/${PATH}/v1").unwrap();
        assert!(expanded.starts_with("https://x/"), "{expanded}");
        assert!(expanded.ends_with("/v1"), "{expanded}");
        assert!(!expanded.contains("${"), "{expanded}");

        assert_eq!(
            expand_environment("https://x/${AX_TESTS_UNSET_VAR}/v1"),
            None
        );
        assert_eq!(
            expand_environment("https://x/v1").as_deref(),
            Some("https://x/v1")
        );
    }

    /// Every provider either yields a request URL or explains why it does not,
    /// and no excluded provider can be re-enabled by saving a credential.
    #[test]
    fn support_and_explanations_cover_the_whole_catalog() {
        let mut supported = 0;
        for spec in PROVIDERS {
            if provider_supported(spec.id) {
                supported += 1;
                assert_eq!(
                    provider_unsupported_reason(spec.id),
                    None,
                    "{} is supported",
                    spec.id
                );
            } else {
                let reason = provider_unsupported_reason(spec.id)
                    .unwrap_or_else(|| panic!("{} is unsupported with no reason", spec.id));
                assert!(!reason.is_empty(), "{}", spec.id);
            }
        }
        assert_eq!(
            supported,
            PROVIDERS.len() - 1,
            "the unsupported set changed; update provider_unsupported_reason"
        );
    }

    /// Discovery used to label every remote model as tool-capable, so an ASR or
    /// TTS entry was offered to the agent as if it could call a tool. The
    /// examples below are catalog entries whose own `supports_tools` flag
    /// agrees with this rule from both directions.
    #[test]
    fn discovered_non_chat_models_are_not_tool_capable() {
        const ENDPOINT: &str = "https://example.test/v1/chat/completions";
        for id in [
            "mimo-v2-tts",
            "mimo-v2.5-tts-voiceclone",
            "voyage/rerank-2.5",
            "text-embedding-3-large",
            "grok-imagine-video",
        ] {
            let info = compatible_model_info(id.to_owned(), "xiaomi", ENDPOINT);
            assert!(!info.supports_tools, "{id} should not be tool-capable");
        }
        for id in ["mimo-v2.5", "glm-4.6v", "grok-4.5"] {
            let info = compatible_model_info(id.to_owned(), "xiaomi", ENDPOINT);
            assert!(info.supports_tools, "{id} should stay tool-capable");
        }
    }
}
