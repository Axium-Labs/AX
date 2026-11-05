<a id="adr-0020-native-model-providers-and-configuration-status"></a>
# ADR 0020：原生模型提供商与配置状态

[English](0020-native-model-providers.md) | **简体中文**

状态：已接受，2026-10-05

<a id="context"></a>
## 背景

AX 带有 pi 提供商清单，但运行时只实现 Chat Completions、OpenAI/Codex Responses 和 WorkBuddy 分发。保存 Anthropic、Google、Vertex、Bedrock、Azure、Cloudflare Gateway 或 Radius 密钥不会凭空生成适配器。Workers AI 已可通过账户 id 工作，但缺失端点字段被误标为不支持。目录和运行时分发逐渐偏离。

<a id="decision"></a>
## 决策

- 在 `ModelProvider` 后新增原生 Anthropic Messages、Gemini/Vertex GenerateContent、Bedrock ConverseStream 和 Radius Pi Messages 模块。Azure 复用 Responses，采用独立标识、资源路由、部署映射及 `api-key`。Cloudflare Gateway 复用 Chat Completions，使用 `/compat` 端点和 `cf-aig-authorization`。
- 运行时、子运行时和发现共用 `provider_adapter` 构造。保留 OpenAI、Codex、DeepSeek 和 WorkBuddy 路径，Workers AI 推理格式不变。
- `supported` 由适配器能力决定。缺失账户／资源字段显示配置原因及目录警告，不隐藏适配器。仅宣称 AX 已实际实现的账户 OAuth 流程。
- 通过官方 SDK 链惰性解析 AWS 凭据，用官方 SigV4 signer 签名，Smithy 对事件帧做 CRC 检查。惰性解析 Google ADC 刷新令牌、服务账户 JWT 或元数据令牌。不复制这些存储中的凭据，启动不调用 API。
- 签名助手块保存在按提供商／模型限定的可选元数据中。同模型回放签名并计入大小预算，旧会话 JSON 默认该字段不存在。不新增模型调用或变换。
- 按字节解析原生流以处理碎片 UTF-8，拒绝截断／错误流，为现有重试策略保留状态码及 Retry-After。
- Radius 经 `/v1/config` 发现目录及推理基础地址，不使用 OpenAI。交互启动时没有 Radius 目录则进入发现／模型选择。

<a id="consequences-and-limits"></a>
## 影响与限制

回环协议／认证测试只证明适配器行为，不证明真实账户权限或余额。Vertex/Bedrock 离线目录仍作回退／缓存。未实现 Anthropic/Radius 浏览器 OAuth 和 Google external-account 工作负载联合 ADC。Vertex 涵盖 Google publisher 模型，Bedrock 涵盖 ConverseStream 模型。GitHub Copilot 仍不受支持。字段见[提供商](../providers.zh-CN.md)。

AWS 依赖锁定为兼容 Rust 1.92。SDK 凭据／签名与 AX 现有推理 HTTP 传输及代理配置共存。

参考：[pi](https://github.com/earendil-works/pi/tree/200387122ca450d6387f033949423114a270b96c/packages/ai/src)、[Cloudflare Unified API](https://developers.cloudflare.com/ai-gateway/usage/chat-completion/)、[Bedrock ConverseStream](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html)、[Gemini signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures)。
