//! Built-in provider metadata ported from pi's MIT-licensed provider catalog.
//!
//! This module deliberately describes authentication and wire protocol
//! separately. Sharing an API-key dialog does not imply that Anthropic,
//! Google, Bedrock, and `OpenAI` use the same request schema.

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
        "deepseek" => "https://api.deepseek.com",
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
