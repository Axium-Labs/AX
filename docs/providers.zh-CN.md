<a id="models-providers--auth"></a>
# 模型、提供商与认证

[English](providers.md) | **简体中文**

本文介绍提供商抽象、凭据、模型发现与选择。

<a id="overview"></a>
## 概览

AX 从本机 `<install-dir>/.ax/auth.json` 及 `DEEPSEEK_API_KEY` 等标准环境变量发现提供商。**启动不发网络请求**，首次模型调用才构建提供商。

`cli::providers` 唯一定义已配置来源：AX `auth.json`、常规环境变量、显式旧 Codex 路径。CLI 启动与 TUI `/model` 刷新共用。

`ax auth remove <provider-id>` 在 `auth.json` 将对应凭据替为作用域 `{"type":"disabled"}`，抑制已存／环境／ambient／旧显式凭据发现，拒绝显式运行选择。不动系统环境及其他提供商。保存新密钥或完成 OAuth 替换标记并重启用。原 TUI 登出只删已存凭据，不作停用。

<a id="provider-abstraction"></a>
## 提供商抽象

`model` 定义中立 `ModelProvider`、消息、调用及响应，`complete_stream` 统一流入口，默认非流回退。报告上下文窗口驱动压缩。`ProviderSpec` 标识厂商、凭据变量、`ProviderProtocol`。DeepSeek、Groq、Mistral、OpenRouter、Together、xAI、Kimi 共用 `OpenAiCompatibleProvider` Chat Completions。`OpenAiCompatibleConfig::new` 接模型／密钥／端点／窗口，DeepSeek 默认在元数据。OpenAI Responses/Codex 保留适配器。

流及非流的 `reasoning_content`、`reasoning`、结构化 `reasoning_details` 留在提供商／模型作用域元数据。同提供商同模型后续助手消息回放，包括无工具最终答案和 Goal 历史。只转这些协议字段，不转任意元数据。结构细节保留 OpenRouter 等兼容网关的不透明数据及签名。上下文除可见文本／参数外预算最大推理表示，遵循 [DeepSeek 思考／工具契约](https://api-docs.deepseek.com/guides/thinking_mode/)。旧消息无元数据不能恢复已丢推理，提供商拒旧历史时需新 Session。

OpenAI/Codex/Azure Responses 在相同持久契约保留有序输出项，含推理、助手消息、函数。请求 `store: false`，只同提供商／模型及 Responses 协议回放。加密推理是不透明状态，不解密或用显示摘要替代，保留表示计预算。见 [OpenAI 推理状态](https://developers.openai.com/api/docs/guides/reasoning)。

两适配器拒流错误及无有效完成边界 EOF。Responses `failed` 或过滤／未说明 incomplete 失败。声明输出 Token 上限可请求既有 Runtime 续接，未完函数不执行。部分输出错不当成功，已输出文本／思考不重放。

原生适配器对 Token 截断且带工具响应也明确失败，未完签名工具块不能安全执行或改写回放。仅文字／推理截断保留原生状态供续接；Gemini `MAX_TOKENS` 映射共享续接，不误当正常完成。

| 提供商 | 端点 | 凭据 |
|---|---|---|
| DeepSeek | Chat Completions | API 密钥 `DEEPSEEK_API_KEY` |
| OpenAI | Responses API | API 密钥 `OPENAI_API_KEY` |
| OpenAI Codex | ChatGPT Codex Responses | 浏览器／设备码 OAuth 或旧 `--codex-auth` |
| OpenAI-compatible 厂商 | 元数据端点 | 已存或环境 API 密钥 |

<a id="native-and-resource-scoped-providers"></a>
## 原生与资源作用域提供商

以下适配器已实现。支持指运行能力，与凭据、资源配置、在线访问分开。缺字段报配置错和目录警告，`_ax/models.catalog[].configuration_reason` 暴露原因，不将已实现标不支持。

| 提供商 ID | 协议 API | 凭据及必需配置 |
|---|---|---|
| `anthropic` | Anthropic Messages SSE | `ANTHROPIC_API_KEY` 或已存密钥，可选 `ANTHROPIC_BASE_URL` |
| `google` | Gemini `streamGenerateContent` SSE | `GEMINI_API_KEY` 或已存密钥，可选 `GOOGLE_API_BASE_URL` |
| `google-vertex` | Vertex Gemini SSE | express 用 `GOOGLE_CLOUD_API_KEY`，或 ADC 加 `GOOGLE_CLOUD_PROJECT`/`GCLOUD_PROJECT`；可选 `GOOGLE_CLOUD_LOCATION`，默认 `us-central1`，支持 `global` |
| `amazon-bedrock` | ConverseStream，AWS 二进制事件 | `AWS_BEARER_TOKEN_BEDROCK` 加地区，或 SDK 凭据链；`AWS_REGION`/`AWS_DEFAULT_REGION` 或 profile 地区 |
| `azure-openai-responses` | Azure Responses | `AZURE_OPENAI_API_KEY` 加 `AZURE_OPENAI_BASE_URL` 或 `AZURE_OPENAI_RESOURCE_NAME` |
| `cloudflare-ai-gateway` | 兼容 `/compat/chat/completions` | `CLOUDFLARE_API_KEY`、`CLOUDFLARE_ACCOUNT_ID`、`CLOUDFLARE_GATEWAY_ID`，使用 `cf-aig-authorization` |
| `cloudflare-workers-ai` | 既有 Chat Completions | `CLOUDFLARE_API_KEY` 加 `CLOUDFLARE_ACCOUNT_ID`，格式不变 |
| `radius` | Pi Messages SSE `/messages` | `RADIUS_API_KEY`，可选 `RADIUS_BASE_URL` 默认 `https://radius.pi.dev`，经 `/v1/config` 发现模型与推理地址 |

`provider_adapter` 由运行构建和目录发现共用，选择、子运行和 TUI `/model` 同分发。CLI `compatible` 只选 Chat Completions；原生 ID 用 `--provider anthropic`、`--provider google` 等。Radius 无缓存时交互启动进入发现／选择，非交互需显式提供商及模型。

<a id="web-search-capabilities"></a>
## 网页搜索能力

所有可运行提供商用 `web(search)`，实际适配器、端点及凭据决定候选，首次服务搜索验证账户／模型。注册提示／模型名非权益保证，兼容网关默认 Remote。

候选含官方 OpenAI/Codex/Azure Responses、Anthropic、Gemini/Vertex、Moonshot、OpenRouter native-only、Groq。Z.AI coding、Qwen Token Plan、Bedrock Converse、Mistral Chat 不同于厂商其他搜索 API，用 Remote。显式通用 Z.AI/Model Studio 有文档化扩展和来源验证。其他可用模型用匿名 Exa/Parallel。无需额外密钥，但模型搜索／推理可计费。见[完整矩阵与官方依据](web-search-audit.zh-CN.md#complete-registry-classification)、[架构](web-search-architecture.zh-CN.md)、[实测限制](web-search-audit.zh-CN.md#execution-evidence-and-remaining-limits)。

Azure 根规范到 `/openai/v1/responses`，也接 API base 或完整路径。`AZURE_OPENAI_DEPLOYMENT_NAME_MAP` 逗号分隔 `model-id=deployment-name`，不改模型身份。用 `api-key`，不用默认 bearer；可选 `AZURE_OPENAI_API_VERSION` 加查询，默认 v1。OpenAI/Codex 原认证请求不变。

Bedrock 惰性用官方 AWS SDK 链：环境／session token、profile、credential process、SSO、web identity、容器及实例。SigV4 用所选地区和 `bedrock` 服务，bearer 同 ConverseStream。模型可基础模型、inference profile、ARN。依赖兼容 Rust 1.92 MSRV。

Vertex ADC 支持 `gcloud auth application-default login` 用户刷新令牌、`GOOGLE_APPLICATION_CREDENTIALS` 服务账户及实例元数据。到期前刷新，ADC quota project 或 `GOOGLE_CLOUD_QUOTA_PROJECT` 作为 `x-goog-user-project`。未实现 external-account 联合 ADC，明确配置错误；不复制 ADC／厂商 CLI 凭据。

原生助手块保存在可选 `Message.provider_metadata`，含签名思考／Gemini signature，经正常检查点持久并计回放预算，仅同提供商模型复用。文本、图像、schema、并行工具结果、流式文本／思考、用量和最终参数映射中立契约。格式坏、截断、错误流失败；429/5xx/Retry-After 保留既有重试分类，不增调用。

Anthropic/Gemini 目录分页，Radius 动态；Vertex/Bedrock 缓存／内置不冒充认证实时发现。Azure/Cloudflare 失败保警告并回退目录。真实权限、地区、余额仅推理确认。

账户 OAuth 只 Codex/WorkBuddy，其他密钥提供商不再显示无效 OAuth。Anthropic/Radius 密钥不意味着浏览器／订阅 OAuth。Vertex 目前 Google Gemini publisher；Bedrock 仅 ConverseStream，非任意 InvokeModel。

参考 [pi 适配器](https://github.com/earendil-works/pi/tree/200387122ca450d6387f033949423114a270b96c/packages/ai/src/providers)、[pi 协议](https://github.com/earendil-works/pi/tree/200387122ca450d6387f033949423114a270b96c/packages/ai/src/api)、[Bedrock](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html)。

新本地模型实现并注册 `ModelProvider`，Loop 保持中立。`ModelProvider::capabilities()` 报 `vision`、`tool_calling`。Responses 将图片映射 `input_image`，已知视觉家族 `gpt-4o`、`gpt-4.1`、`gpt-5`、`gpt-6` 启用。Anthropic/Gemini/Vertex 用原生图像，Bedrock 对 Claude 启用。DeepSeek／通用兼容在未建立专属图像格式及能力前只文本，图像部件与纯文本对话分开保存。

<a id="credentials"></a>
## 凭据

位于 `<install-dir>/.ax/auth.json`，或设置后的 `$AX_HOME/auth.json`：

```json
{
  "deepseek": { "type": "api_key", "key": "sk-..." },
  "openai-codex": {
    "type": "oauth",
    "access": "eyJ...",
    "refresh": "...",
    "expires": 1234567890,
    "account_id": "org-..."
  }
}
```

- **权限**：Unix `0600`，Windows `icacls` 去继承 ACE，给当前用户完全控制，尽力执行，保存不依赖其成功。
- **顺序** `AuthStorage::resolve_api_key`：AX 存储先于常规环境，OAuth 用 `resolve_oauth`。
- 旧版项目目录凭据经 `migrate_legacy_project_auth` 一次复制到 `<install-dir>/.ax/auth.json`。不导入其他应用，尤其 `~/.codex/auth.json`，除非显式 `--codex-auth`。
- `<install-dir>/.ax/auth.json` 临时文件加 rename 原子写。

<a id="model-discovery"></a>
## 模型发现

在 `<install-dir>/.ax/models/` 动态发现，内置目录加逐提供商 `models/<provider>.json` 刷新缓存，避免每次启动 API；刷新 15 秒超时回退缓存。

`_ax/models` 只公布各模型 `reasoning_efforts`、`default_reasoning_effort`。`none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`、`ultra` 跨目录／CLI／序列化原值保留，`minimal` 不再变 `low`，`ultra` 不再变 `max`。不为未声明模型增加等级。Crew 组合面板只消费目录子集，不虚构选项。

<a id="model-selection"></a>
## 模型选择

- **`/model`** 选择已配置模型及支持的推理，可带如 `/model provider/model-id`。精确缓存匹配直接应用，否则把参数作搜索打开选择器并后台刷新，只列已配置提供商。
- 上次提供商／模型／强度保存到 `<install-dir>/.ax/config.json` 的 `AxConfig { model: { provider, model, reasoning_effort } }`，下启动恢复，旧 `config.toml` 自动迁。
- CLI `--provider`、`--model`、`--codex-auth`、`--context-window` 高于保存选择，后者高于本地检测。
- `--provider` 接目录 ID 或 `deepseek` / `openai` / `codex` / `compatible`，可精确选 `xiaomi-token-plan-cn`、`xiaomi-token-plan-sgp`、`xiaomi-token-plan-ams`，与 `xiaomi` 同模型 ID 不同端点。多个兼容凭据时仅 `compatible` 有歧义。
- 多提供商同裸 `--model` 先按本地凭据筛，仍多则要求 `--provider`，不猜。

<a id="auth-flows"></a>
## 认证流程

- **`/login`** 连接提供商。密钥用 `SecretInput` 并经 `AuthStorage` 保存。Codex 先选浏览器／设备码再后台授权，UI 收 URL、完整设备码及完成状态。Esc、退出或登出取消等待。存密钥后使 runtime 失效并异步刷新，`/login` 不选模型，需 `/model`。
- **`/logout`** 删除 AX 所存所选凭据并使 runtime 失效，不改环境或厂商 CLI 凭据。
- **Codex 刷新** `refresh_codex_credential_if_needed` 在开始前预留 60 秒到期余量刷新。

<a id="codex-browser-and-device-code-login"></a>
### Codex 浏览器与设备码登录

`ax auth login openai-codex` 默认浏览器，可显式：

```sh
ax auth login openai-codex --method browser
ax auth login openai-codex --method device
```

浏览器打印可开／复制 URL，PKCE S256 加随机 state，只监听 `127.0.0.1:1455`，注册回退 `1457`，路径 `/auth/callback`，交换前检查 state，完成／取消释放监听。两种均 15 分钟过期。端口占用或浏览器无法回调时用设备码，可能需账户／工作区启用设备码授权，见 [OpenAI 认证](https://learn.chatgpt.com/docs/auth)。均存 AX 提供商凭据，不动 Codex 缓存。

`--json` 为桌面每 stderr 行发 JSON 进度，含 `type`（`prompt`/`notice`）、`message`、`url` 字符串／null、`code` 字符串／null。设备码逐字取认证响应，不预设长度／标点；浏览器无设备码。进程退出决定完成，错误仍 stderr 诊断，不发 Token 或 PKCE verifier。WorkBuddy 也接 `--json`，`--method` 只 Codex，`--region` 只 WorkBuddy。

<a id="reference"></a>
## 参考

| 职责 | 代码 |
|---|---|
| 抽象及提供商 | `crates/model/src/lib.rs`、`crates/model/src/providers.rs`、`adapter.rs`、`native/`、`openai_compatible.rs`、`openai.rs`、`codex_device.rs`、`codex_browser.rs` |
| 凭据 | `crates/model/src/auth.rs` |
| 模型目录 | `crates/model/src/registry.rs` |
| 配置 | `crates/cli/src/config.rs` |
| 检测／CLI 解析 `ModelSelection`、`ProviderKind` | `crates/cli/src/providers.rs`、`crates/cli/src/model_selection.rs` |
| `/login`、`/logout`、`/model` | `crates/cli/src/tui/commands.rs` |

<a id="catalog-and-connection-status-in-crew"></a>
## Crew 目录与连接状态

`crates/model/src/catalog.json` 包含现有公开 pi 离线元数据，不含端点覆盖，空 AX home 不需另复制 `pi-catalog.json`。优先实时、再缓存、再安装／内置 bootstrap。列表失败即使离线可选也保警告，离线不证明认证、余额或权限。

内置端点优先缓存端点，旧 MiniMax Anthropic 或缺 `/v1` Fireworks 不会重定向 Chat 适配器。提供商、地区、订阅均属身份，不静默将密钥发别家。

Crew 读 `_ax/models` 的完整 `catalog`，不维护副本或读缓存；旧 AX 无字段提示更新。`_ax/refresh-models` 返 `source`（`live`、`cache`、`fallback`）和 `warning`。存密钥发现该提供商，显式刷新全部已配且支持项，含已有缓存。只提供支持工具的模型；UI 区分保存凭据、本地模型和成功在线发现，发现非推理测试。GitHub Copilot 明确禁用，资源配置错与不支持分开。

端点参考 [MiniMax OpenAI SDK](https://platform.minimax.io/docs/api-reference/text-openai-api)、[Fireworks](https://docs.fireworks.ai/api-reference/post-chatcompletions)。

<a id="native-workbuddy"></a>
## 原生 WorkBuddy

`ax auth login workbuddy` 展示可点／复制 URL，等待授权，在既有 `AuthStorage` 存 access/refresh 和到期，完成账户模型发现后退出。TUI `/login` → 账户登录提供，后用 `/model` 选 `workbuddy/<model-id>`，CLI 也可 `--provider workbuddy --model <id>`。地区身份、凭据及缓存分离：

| 地区 | 提供商／凭据键 | 命令 | 模型／认证端点 |
|---|---|---|---|
| 国际 | `workbuddy` | `ax auth login workbuddy --region intl` | `https://www.workbuddy.ai` |
| 中国 | `workbuddy-cn` | `ax auth login workbuddy --region cn` 或 `ax auth login workbuddy-cn` | `https://copilot.tencent.com` |

原 `ax auth login workbuddy` 仍国际。`/login` 分 **WorkBuddy International**、**WorkBuddy China**，`/model` 及保存用 `workbuddy/<id>`／`workbuddy-cn/<id>`。可同时登录，重登仅清对应区缓存，中国不回退国际凭据，已知 token 域／issuer 区域不匹配先拒。`workbuddy-cn --region intl` 冲突拒绝。中国请求使用产品 UA、`www.codebuddy.cn` Origin/Referer、`copilot.tencent.com` 域和 `X-Auth-Refresh-Source: workbuddy`，国际保留 `X-Auth-Refresh-Source: plugin`。

协议参考 [workbuddy2api-hub](https://github.com/ardeyouxipianyi/workbuddy2api-hub)，commit `6c2a6637f27dcecb6c4956392dfd936fac687306`，2026-09-30 验证。当前是服务随机 state、浏览器授权和 Token 轮询，**不是**本地 authorization-code 回调。检查 HTTPS origin、登录路径和精确 state，拒重复 state 参数，仅轮询本次。参考及当前页面无 localhost 回调或 PKCE 契约，使用已约定轮询，不声称实现不支持项。临时状态内存，10 分钟到期，CLI Ctrl+C、TUI Esc 或退出取消。

端点：

- `POST /v2/plugin/auth/state?platform=CLI` → `data.state`、`data.authUrl`。
- `GET /v2/plugin/auth/token?state=...`：`11217` 为等待，成功 `data` 含 `accessToken`、`refreshToken`、到期（显式 epoch、JWT `exp` 或 `expiresIn`）。
- `POST /v2/plugin/auth/token/refresh`：`X-Refresh-Token`、各区 `X-Auth-Refresh-Source`，兼容 `data`、`data.data`。
- 国际 `GET /v2/enterprises/personal/models`，中国 `GET /v3/config`，去重 `data.agents[].models`，也接对象 agents，省非聊天 `lite`。
- `POST /v2/chat/completions`：直接发 AX 消息／工具，非流调用累积 SSE。共享解析支持碎片 UTF-8、LF/CRLF、推理／工具 delta、重复 `data:`、`[DONE]`。

每模型／列表请求重载凭据，按到期刷新；跨 provider 实例串行更新，使用前持久轮换 Token。401 终态，需重新授权，不重放失败请求；403 终态。429 保 Retry-After 进重试，403/429 都不刷凭据。厂商正文脱敏，含 Token 响应不泄漏。

不虚构离线模型，发现成功用现有缓存／选择器，刷新失败留缓存警告。新账户登录先清旧 WorkBuddy 缓存再发现。目录只是账户声明，不保证后续额度／权限不变。

产品 UA／路由头为上游协议要求。不发机器／会话指纹、浏览器 cookie，不导入桌面 token，不做代理轮换、账户池、签到／奖励，不使用 WorkBuddy Agent Session、技能／工具或注入系统提示；对话和 schema 仅 AX 提供。

<a id="crew-account-sign-in"></a>
### Crew 账户登录

ACP 目录含 `auth_kind`（`api_key`、`oauth`、`ambient`）。Crew 分账户、密钥、环境，拒为 OAuth 保存密钥。`ax auth login openai-codex` 将浏览器／设备流提供桌面，用户选方式后用 `--method`、`--json`，展示 URL 和完整设备码，仅点击才开系统浏览器。关对话框取消进程。Token 交换、保存和发现唯一归 AX。

<a id="provider-retry-policy"></a>
## 提供商重试策略

Runtime 先分类。400/401/403、除 408/425/429 外其他 4xx、配置坏、响应坏为终态。408、425、429、5xx、超时、连接重置／中止可重试。可见文本／思考已流出则不重试，防重复。按已存到期刷新仍在发送前，认证请求失败不靠重放来续凭据。

`Retry-After` 秒或 HTTP-date 优先，所有可重试非 2xx 含刷新／目录都保头。否则有上限指数退避加 full jitter。次数／耗时双预算，含剩余时限超时；服务等待超预算则停止，不缩短等待。成功重置序列，传输重试不加模型步／任务数。

```json
{
"retry": { "max_attempts":5, "time_budget_ms":30000,
           "base_delay_ms":200, "max_delay_ms":5000 }
}
```

`max_attempts` 含首请求，0/1 禁重试，大于 5 限 5。缺配置用集中默认，替代旧 Goal 专属一次重试。普通轮／子同策略。

连接预算也限制首请求无文本／思考阶段。开始输出后健康流可超预算，部分响应不自动重放。超时、终态错或 Retry-After 超剩余可少于五次停止。每模型步发 `ModelConnection`，含 `connecting`、`retrying`、`connected`、`failed`、实际次数、有效最大及可选错误。首输出或成功完成发 Connected，CLI/TUI 展重连计数，ACP 转发 Crew。
