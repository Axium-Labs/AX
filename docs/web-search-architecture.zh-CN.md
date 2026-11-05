<a id="web-search-architecture"></a>
# 网页搜索架构

[English](web-search-architecture.md) | **简体中文**

AX 保留统一 `web(search)` 接口。内核工具绑定将其连接到所选具体模型适配器，含已注册工具和 Mod。子代理模型覆盖会重新绑定原生能力；配置客户端、远程会话和远程健康状态在子 fork 间保留。

生产路径：

```
Agent Loop -> permission check -> WebTool::search_many
  -> NativeFirst -> ModelProvider native API/auth/server tools
  -> on unsupported/error/timeout: SearchRouter -> Exa / Parallel MCP (Keenable MCP last)
  -> citations/evidence in Tool result -> ordinary Agent model generation
```

原生适配器实现真实 Responses、Messages、Gemini grounding 及特定文档化 Chat 扩展。候选取决于实际 API 端点、适配器和凭据形式，再经服务端执行及来源验证。不猜模型名称，也不认为兼容 API 普遍享有原生搜索权限。完整[提供商审计](web-search-audit.zh-CN.md)列出清单与协议证据，包括不支持厂商其他产品搜索功能的 API。

默认 Remote 为厂商授权匿名 Exa 和 Parallel MCP，惰性初始化并发现 schema。匿名 Keenable MCP 始终排在两者之后作为回退。不需要额外搜索密钥；可选 Brave/Bocha 仅在用户提供搜索密钥后参与路由。共享调度器按失败历史及观测延迟独立排名，对冲最多两请求，遵守 Retry-After，冷却后恢复。Native 先于 Remote，不对模型账户搜索进行对冲。

查询任务预算为 30 秒，原生尝试最多 12 秒或预算一半，远程连接／读取／总超时默认 5/10/15 秒。首次原生失败跨查询合并。不重放请求；丢弃任务取消待处理读取／续接。远程会话 404 使缓存会话失效，供下一任务重建，不重复失败搜索。

重复查询及规范化 URL 合并来源／引用。结果仍是不可信外部数据，不能授予工具权限。托管搜索访问的网站在远端执行，因此域名策略在联网前无法满足时拒绝操作。`web(fetch)`、Browser、MCP 和 Skill 保留各自执行及权限边界。

服务中断准确报错，没有保证可用的公开回退，也不解析 HTML 搜索引擎。参见[快速开始](web-search-quickstart.zh-CN.md)、[实现](web-search-implementation-guide.zh-CN.md)及[测试证据与剩余限制](web-search-audit.zh-CN.md#execution-evidence-and-remaining-limits)。
