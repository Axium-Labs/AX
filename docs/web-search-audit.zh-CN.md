<a id="web-search-production-audit"></a>
# 网页搜索生产审计

[English](web-search-audit.md) | **简体中文**

于 2026-10-10–11 审查当前注册表、模型目录／发现、凭据解析、适配器工厂、内核、子组合、CLI 和 ACP。区分协议候选与已证明账户／模型访问。

<a id="production-path"></a>
## 生产路径

CLI/TUI/ACP 创建所选 `ModelProvider` 和 `WebTool`。`AgentKernel::new`/`register_tool` 经 `Tool::bind_model` 绑定，Mod 转发 hook。普通 Agent Loop 检查权限后，`WebTool::search_many` 为各唯一查询运行 `NativeFirst`。原生搜索使用适配器真实协议和认证，成功服务端搜索将来源及保留的平台证据作为普通工具结果，同一循环交给所选模型生成最终答案。研究生成独立于父对话，不会把服务端工具误作 AX 客户端函数调用。

原生不支持、拒绝、格式错、无来源或超时则由 `NativeFirst` 转共享 `SearchRouter` 的 Exa/Parallel MCP，保留延迟排名、有界对冲、熔断和取消。子 fork 保留配置客户端、路由状态、搜索会话；覆盖模型重新绑定子原生能力，远程健康仍共享。模型对冲不重复原生搜索。AXCrew 使用同一 ACP，无需 Crew 源码修改，测试真实 ACP 协议路径，非桌面 UI。

<a id="capability-rules"></a>
## 能力规则

`SearchCapability::ProtocolCandidate` 是注册提示，不是支持证明。路由读 `ModelProvider::native_search_protocol`，不看注册 ID／模型名，检查 HTTPS 主机、API 路径、适配器及凭据形式。兼容网关不默认有 OpenAI 搜索权益。响应须有可用 HTTP(S) 来源及完成／执行证据。首账户／模型探测串行，失败使后续探测冷却。更换模型重置原生状态；不支持 400/404/422 停止该绑定后续尝试，401/403/429 及重复失败冷却，保留 Retry-After。Remote 仍要求模型推理凭据有效，不能修复过期登录。

动态目录 ID 使用同适配器和服务验证，不需静态模型家族白名单。新增自定义适配器默认 Remote，或实现两个原生 trait 方法对接有文档协议。当前 CLI 没有自定义提供商注册，此审计不重新引入 2026-10-09 移除功能。

<a id="complete-registry-classification"></a>
## 完整注册表分类

以下为本 checkout 完整清单。“候选”始终取决于实际端点、模型、地区、账户权限。全部可运行项有 Remote 回退；Copilot 仅目录，不能在本版本执行 AX Agent。

注册项 **43** 个，含一个不可运行项。

| 提供商 ID | 当前 AX API | 认证 | 原生／回退依据 |
|---|---|---|---|
| `workbuddy-cn` | Managed | ExternalOAuth | Remote：中国区独立 OAuth／目录传输，无已验证服务端搜索契约。 |
| `workbuddy` | Managed | ExternalOAuth | Remote：国际区托管 OAuth 聊天传输无文档化服务端搜索扩展。 |
| `amazon-bedrock` | Bedrock | Ambient | Remote：当前 ConverseStream 无搜索契约，Mantle Responses 是不同 API。 |
| `ant-ling` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `anthropic` | Anthropic | ApiKey | 候选：官方 Messages／密钥／服务端 web_search，自定义网关用 Remote。 |
| `azure-openai-responses` | OpenAiResponses | ApiKey | 候选：官方 Azure Responses 部署／api-key，请求验证权限及地区。 |
| `baseten` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `cerebras` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `cloudflare-ai-gateway` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `cloudflare-workers-ai` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `deepseek` | OpenAiCompatible | ApiKey | Remote：AX 实际 Chat Completions 支持客户端函数，已实时测试。 |
| `fireworks` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `github-copilot` | Managed | ExternalOAuth | 不可运行：注册项无 AX 运行适配器，不能执行 Agent。 |
| `google` | Google | ApiKey | 候选：官方 Gemini API／密钥／google_search，自定义网关用 Remote。 |
| `google-vertex` | Google | Ambient | 候选：Vertex publishers/google 端点，既有 ADC 或 express key 及地区。 |
| `groq` | OpenAiCompatible | ApiKey | 候选：官方全球 Chat browser_search，需实际工具执行／来源回执。 |
| `huggingface` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `kimi-coding` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `meta` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `minimax` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `minimax-cn` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `mistral` | OpenAiCompatible | ApiKey | Remote：搜索需 Conversations/Agents，非当前 Chat Completions。 |
| `moonshotai` | OpenAiCompatible | ApiKey | 候选：官方 Moonshot 内置 $web_search 及 echo 续接。 |
| `moonshotai-cn` | OpenAiCompatible | ApiKey | 候选：独立官方中国区端点／密钥，同内置协议。 |
| `nvidia` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `openai` | OpenAiResponses | ApiKey | 候选：官方 Responses，需 completed web_search_call 及引用。 |
| `openai-codex` | OpenAiResponses | CodexOAuth | 候选：实际 ChatGPT Codex Responses OAuth，不是 OpenAI API 密钥访问。 |
| `opencode` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `opencode-go` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `openrouter` | OpenAiCompatible | ApiKey | 候选：官方 API 插件仅 engine=native，无付费远程引擎回退。 |
| `qwen-token-plan` | OpenAiCompatible | ApiKey | Remote：Token Plan Chat/Harness 契约不同于通用 Model Studio enable_search／来源 API。 |
| `qwen-token-plan-cn` | OpenAiCompatible | ApiKey | Remote：Token Plan Chat/Harness 契约不同于通用 Model Studio enable_search／来源 API。 |
| `qwen-token-plan-individual` | OpenAiCompatible | ApiKey | Remote：Token Plan Chat/Harness 契约不同于通用 Model Studio enable_search／来源 API。 |
| `radius` | PiMessages | ApiKey | Remote：当前 Pi Messages 适配器无已验证服务端搜索契约。 |
| `together` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `vercel-ai-gateway` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `xai` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions，旧 Live Search 已退役，Responses 是不同协议。 |
| `xiaomi` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `xiaomi-token-plan-ams` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `xiaomi-token-plan-cn` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `xiaomi-token-plan-sgp` | OpenAiCompatible | ApiKey | Remote：当前 Chat Completions 适配器无已验证服务端搜索扩展，不从模型名推断。 |
| `zai` | OpenAiCompatible | ApiKey | 默认 Remote：coding 端点。覆盖为普通官方 /api/paas/v4 时才是有条件原生候选。 |
| `zai-coding-cn` | OpenAiCompatible | ApiKey | 默认 Remote：中国区 coding 端点，普通 BigModel API 是独立接口。 |

<a id="official-protocol-evidence"></a>
## 官方协议证据

- [OpenAI Responses](https://developers.openai.com/api/docs/guides/tools-web-search)：`web_search`、完成搜索回执及 URL 注释／action 来源。Codex OAuth 使用既有 ChatGPT Responses 和 preview 工具，不把 ChatGPT OAuth 当普通 API 密钥。
- [Anthropic Messages](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool)：`web_search_20250305`、服务结果块、引用、加密内容和用量。暂停／不完整响应回退，不无界续接。
- [Gemini grounding](https://ai.google.dev/gemini-api/docs/google-search)：`google_search`、grounding chunk、support 和搜索入口。Vertex 复用 API 密钥／ADC 刷新、quota project、地区端点。
- [Azure](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/web-search)：真实 Azure Responses 部署及 `api-key`，地区／管理员拒绝则回退，模型名匹配不授部署支持。
- [Moonshot](https://platform.kimi.ai/docs/guide/use-web-search)：内置 `$web_search`、不变助手推理／工具参数、一次 echo 续接。Markdown 引用标为内置答案引用，不虚构 OpenAI 注释。
- [OpenRouter](https://openrouter.ai/docs/guides/features/plugins/web-search)：显式 `engine: native`，保留 URL 注释。不选择 auto、Exa、Parallel 或其他消耗 OpenRouter 额度的搜索引擎。
- [Groq](https://console.groq.com/docs/tool-use/built-in-tools/browser-search)：要求 `browser_search`、实际工具／浏览器／搜索结果及引用。不支持模型和地区／主权端点用 Remote，不选已移除 Compound 模型替代。
- [Z.AI](https://docs.z.ai/guides/tools/web-search)：普通 `/api/paas/v4` Chat 搜索及 `web_search` 来源。AX 默认 `/api/coding/paas/v4` 不继承能力，不静默切换付费通用 API。
- [Model Studio](https://docs.modelstudio.console.alibabacloud.com/en/model-studio/web-search)：通用 API `enable_search` 及实际来源元数据。AX 的 [Token Plan Harness](https://www.alibabacloud.com/help/en/model-studio/token-plan-harness-tool) 是独立自动工具契约，不根据通用文档错误分类，当前 Token Plan Chat 用 Remote。
- [Mistral](https://docs.mistral.ai/studio/agents/agent-tools/websearch) 需 Conversations/Agents，不是 AX Chat Completions。[Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/web-search.html) 搜索用 Mantle Responses，不是当前 ConverseStream。[DeepSeek Chat](https://api-docs.deepseek.com/api/create-chat-completion/) 支持客户端函数，所以用 Remote。

原生账户搜索可能产生提供商推理／搜索用量，工具指引和快速开始已说明。无附带私有密钥，可选 Brave/Bocha 需用户显式搜索密钥。

<a id="remote-service-authorization"></a>
## Remote 服务授权

[Exa MCP](https://exa.ai/mcp) 说明免密钥使用 `https://mcp.exa.ai/mcp`。[Parallel 公告](https://parallel.ai/blog/free-web-search-mcp)及 [MCP 文档](https://docs.parallel.ai/integrations/mcp/search-mcp) 授权匿名访问 `https://search.parallel.ai/mcp`。[Keenable MCP](https://docs.keenable.ai/mcp-server) 在 `https://api.keenable.ai/mcp` 共享公开层提供免密钥访问；AX 用 `X-Keenable-Title: AX` 标记，并仅在前两者之后尝试。不发送 API 密钥、账户 cookie、付费 SKU 或伪造模型身份。初始化 MCP、协商支持版本、发现当前 schema，再调用声明搜索工具。Exa 当前必填 `objective` 通过发现获取，不假定旧例。复用会话及 Parallel conversation ID。过期下任务重建，不重放失败搜索。

第三方服务有网络／匿名限流，AX 不保证可用；全不可用准确报错。显式结构化空结果数组是真实空搜索，不虚构来源假成功。来源相关性可能随后端变化，用户应优先权威来源，必要时 fetch。

<a id="repairs-and-safety"></a>
## 修复与安全

已移除全部 DuckDuckGo 页面抓取、DOM／重定向提取、SearXNG 适配、配置／超时及旧测试，移除仅供搜索的直接 DOM 依赖；`web(fetch)` 仍保留 HTML-to-Markdown。移除提供商／环境猜测和空原生返回，以真实行为替代错误的“完整／始终可靠”说明。

原生单尝试最多 12 秒，总查询默认 30 秒，始终为 Remote 留时间。远程连接／读取／总默认 5/10/15 秒，429 遵守 Retry-After，各后端失败／延迟独立。OpenAI/Azure 研究输出最多 2048 Token、内置工具最多一次；独立 Codex 订阅传输刻意不传这些控制。其他原生研究输出最多 2048，Moonshot 最多一次续接。保留完整答案及平台原证据，逐来源摘录有界，不为每 URL 重复答案。每查询最多两 Remote 在途。丢任务／对冲取消本地 future、读取及待续接，不启动脱离搜索或传输重放，但服务端可能已接工作。

TLS／代理遵循 reqwest 环境／系统配置。搜索拒绝重定向，清理 HTTP 错误。MCP/JSON 响应及累积原生事件限 2 MB，共享原生 SSE 另限单待帧 8 MiB。重复查询／规范 URL 去重，合并每个已完成重复的引用／证据来源。输出标记 `untrusted_external_data`，模型指引拒绝检索指令，帮助保留权限，不宣称完美抵抗语义注入。域名受限搜索在任一提供商调用前拒绝，因为不能本地约束远程访问网站。已知 URL fetch 保留独立重定向／域名检查。

<a id="execution-evidence-and-remaining-limits"></a>
## 执行证据与剩余限制

协议支持不证明每账户、模型、地区实时权益。Remote 受服务和网络限制，DeepSeek 用 Remote，不证明原生路径。不普遍宣称提供商验收或可达。

代理环境改变需重建客户端或重启 AX，不热加载已有模型／Remote 连接。已知 URL fetch 保留自身权限及重定向检查。

维护者 fixture 和机器特定验证结果属于本地日志，不是公开命令或当前产品保证。公开测量工具及边界见[运行时基准](runtime-benchmarks.zh-CN.md)和 [Agent 基准](agent-benchmarks.zh-CN.md)。
