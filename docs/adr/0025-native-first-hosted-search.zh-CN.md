<a id="0025-native-first-hosted-search"></a>
# 0025：原生优先的托管搜索

[English](0025-native-first-hosted-search.md) | **简体中文**

状态：已接受，2026-10-11。取代 ADR 0019 中的后端选择。

<a id="decision"></a>
## 决策

保留 `web(search)`、现有 Agent Loop／远程调度器。在构建内核及更换子模型时，将实际模型适配器绑定到工具。适配器负责原生 API、认证及来源验证；兼容接口不意味着支持 OpenAI 搜索。原生响应成为普通后续 Agent 生成使用的工具证据。

回退使用匿名、厂商授权的 Exa/Parallel MCP，进行真实 schema 发现，采用有界请求及独立共享健康状态。显式用户密钥 Brave/Bocha 仍可选。移除 HTML 引擎抓取和 SearXNG。原生搜索先于 Remote 顺序执行，模型对冲不会重复原生搜索。

<a id="consequences"></a>
## 影响

所有可运行模型无需单独搜索密钥即可共用 Remote 访问。原生支持取决于账户、模型和端点，必须通过实际服务端执行验证。搜索有取消／截止限制，保留引用来源。服务端域名受限搜索无法满足条件时拒绝返回。不保证匿名服务可用性及语义提示注入抵抗能力。

参见[审计](../web-search-audit.zh-CN.md)及[架构](../web-search-architecture.zh-CN.md)。

修订，2026-10-29：匿名 Keenable MCP（`https://api.keenable.ai/mcp`）作为第三个厂商授权 Remote。它是路由器回退候选，仅在 Exa、Parallel 失败、结果不足或冷却中时尝试；即使观测延迟更低也不会排在前两者之前。请求携带文档规定的 `X-Keenable-Title: AX` 客户端标签，不带密钥。与其他 Remote 一样，端点设为 `None` 即禁用。
