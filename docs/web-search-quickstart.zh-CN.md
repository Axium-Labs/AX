<a id="web-search-quickstart"></a>
# 网页搜索快速开始

[English](web-search-quickstart.md) | **简体中文**

使用任一已配置 AX Agent 模型请求网页研究。可用时 AX 调用所选提供商实际原生协议，否则用匿名 Exa 或 Parallel MCP，最后回退匿名 Keenable MCP，无需额外搜索密钥。模型推理仍需正常凭据，原生搜索可能消耗该模型账户付费推理／搜索额度。匿名 Remote 受服务限制及网络可用性影响，AX 不附带私有密钥。

工具示例保留现有参数别名：

```json
{"operation":"search","queries":["Rust ownership official docs","Rust borrow checker"],"limit":3,"target_results":4}
```

```json
{"operation":"fetch","url":"https://doc.rust-lang.org/book/"}
```

`results` 结构保留 title/url/snippet/source/matched_queries，新增 `evidence` 表达引用、grounding 及重复来源。信封标记 `content_trust: untrusted_external_data`，绝不执行来源中的指令。

可选环境配置，时间为正整数毫秒：

- `AX_SEARCH_NATIVE_TIMEOUT_MS`（12000）、`AX_SEARCH_TASK_TIMEOUT_MS`（30000）。
- `AX_SEARCH_EXA_URL`、`AX_SEARCH_PARALLEL_URL`、`AX_SEARCH_KEENABLE_URL`：显式指定 MCP 端点。
- `AX_SEARCH_<EXA|PARALLEL|KEENABLE|BRAVE|BOCHA>_CONNECT_TIMEOUT_MS`（5000）、`_READ_TIMEOUT_MS`（10000）、`_TIMEOUT_MS`（15000）。
- `BRAVE_SEARCH_API_KEY` / `BOCHA_SEARCH_API_KEY`：可选用户付费服务；提供密钥即明确启用该服务并采用正常计费。
- 标准 HTTP(S)/ALL_PROXY 和 NO_PROXY 配置遵循 reqwest 代理行为。

客户端构建时读取代理环境，修改后需重启 AX；嵌入方可替换显式客户端，重建 Remote 会话。原生传输归所选模型适配器，修改代理时须随适配器重建。

嵌入方可将 `SearchConfig` 端点设为 `None` 禁用某 Remote。不支持的原生端点使用 Remote；模型推理／登录 401 仍需有效模型凭据。429 进入冷却，不重放搜索。所有服务失败时报告真实错误，修复网络／配置或稍后重试。已知 URL 仍可 fetch。已移除旧引擎设置及 `AX_PROVIDER_ID` 不参与路由。

参见[架构](web-search-architecture.zh-CN.md)及[已验证执行限制](web-search-audit.zh-CN.md)。
