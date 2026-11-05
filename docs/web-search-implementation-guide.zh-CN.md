<a id="web-search-maintenance"></a>
# 网页搜索维护

[English](web-search-implementation-guide.md) | **简体中文**

权威[架构](web-search-architecture.zh-CN.md)和[提供商审计](web-search-audit.zh-CN.md)取代旧占位路线图。

- `model::ModelProvider` 拥有 `native_search_protocol` 和 `native_search`。默认方法让新／自定义适配器兼容 Remote。在适配器边界增加原生扩展，提供官方 API 证据及实际来源／完成验证；不要修改核心路由器猜测厂商 ID。
- `model::search` 负责来源／证据契约及有界原生传输。原生协议适配器复用现有认证及流解析器。
- `tool::WebTool` 负责批量、规范化去重和权限处理，绑定／fork hook 保留所选模型和远程状态。
- `tool::web::NativeFirst` 负责原生资格、探测冷却及任务预算。
- `tool::web::remote` 负责匿名 MCP 协议、协商会话、公布的输入 schema 及真实 Exa/Parallel/Keenable 结果格式。
- `SearchRouter` 负责独立远程健康、冷却、延迟、对冲及 future 取消，仍支持注入提供商／客户端。
- 内核构建及子模型覆盖绑定实际提供商；Mod 必须转发绑定／fork hook，不能丢弃被包装工具。

维护者回归 fixture 调用生产执行器，不使用公开绕过标记；网关能力仍为 false。注册表分类构建每种适配器，运行时验证使用实际内核和 SubagentManager。私有 fixture 不作为正式源码依赖或公开 Cargo target。实时验证必须显式执行，使用现有合法凭据或匿名公开 MCP，绝不内置密钥；还使用配置提供商验证真实 CLI ACP 协议结果。

OpenAI 实时验证需要合法 API 密钥，产生正常提供商用量；失败不能描述为原生通过。`AX_HOME` 可指向已有凭据主目录，实时 CLI 数据应放隔离目录。仍需编译、格式、Clippy、路由／抓取回归和平台测试。本地日志明确记录失败及未测试账户／地区环境。当前提供商和网络限制见审计。
