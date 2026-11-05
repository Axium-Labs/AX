<a id="mcp"></a>
# MCP

[English](mcp.md) | **简体中文**

AX 将 Model Context Protocol 服务作为普通工具集成。本文介绍配置、传输、惰性连接及 `/mcp` 命令。

<a id="overview"></a>
## 概览

实际需要服务工具前，`mcp` crate 只读取服务**配置元数据**。实现 stdio、Streamable HTTP、WebSocket 三种传输，支持 `initialize` 协商、现代无状态协议、分页 `tools/list`、`tools/call`、请求超时和进程清理。

每个受限 stdio 传输拥有独立 SandboxManager 和 broker/PID 命名空间，与工具共享工作区／缓存文件，但 shell 工具进程不能从 procfs 读取服务凭据。网络遵循策略：workspace 允许，strict 强制拒绝，见[安全](security.zh-CN.md)。Strict 也禁止 stdio MCP 服务联网。

<a id="configuration"></a>
## 配置

MCP 服务配置位于 `$AX_HOME/mcp.toml` 和 `<project>/.ax/mcp.toml`，由[作用域能力注册表](capabilities.zh-CN.md)加载：项目同 ID 服务替换全局，无关全局项保留。`--mcp-config` 只替代项目来源。项目屏蔽及显式启停存于 `.ax/config.toml`，不改变全局配置。

文件缺失表示空注册表，不是错误，新安装只是尚无服务配置。发布归档带 `mcp.example.toml`，安装器仅在无已有配置时复制到 `$AX_HOME/mcp.toml`，其中全部服务禁用。服务本身是需另装的外部程序，AX 只提供客户端，不附带服务。

参见仓库根 `mcp.example.toml`：

```toml
[servers.local-files]
transport = "stdio"
command = "your-mcp-server"
args = ["--stdio"]
description = "Local filesystem indexer and search"
capabilities = ["search", "read"]
protocol_version = "2026-07-28"
request_timeout_secs = 30
enabled = false

[servers.remote]
transport = "http"
url = "https://example.com/mcp"
headers = { Authorization = "Bearer replace-me" }
description = "Project issue tracker"
capabilities = ["issues", "tasks"]
enabled = false
```

| 字段 | 含义 |
|---|---|
| `transport` | `stdio`、`http` 或 `websocket` |
| `command` / `args` | stdio 传输启动的进程 |
| `url` | http / websocket 服务端点 |
| `headers` | 静态请求头 |
| `description` | *无需连接*即通过能力目录展示给模型 |
| `capabilities` | 声明能力，同样无需连接即可查看 |
| `protocol_version` | 协商的 MCP 协议版本 |
| `request_timeout_secs` | 单请求超时 |
| `enabled` | 默认 `true`，`false` 保留定义但休眠 |

连接前即暴露 `description` 与 `capabilities`，模型可先了解休眠／禁用服务提供什么，再为 `list_tools`/`call` 建立真实连接。

<a id="lsp-server"></a>
### LSP 服务

代码智能需安装暴露 LSP 操作的 MCP 服务，并按其文档命令参数加入 `mcp.toml`：

```toml
[servers.lsp]
transport = "stdio"
command = "your-lsp-mcp-server"
args = ["--your-server-options"]
description = "Language-server code intelligence for this project"
capabilities = ["lsp", "definitions", "references", "diagnostics"]
request_timeout_secs = 300
```

将命令参数替换为对应服务要求。AX 不自行启动语言服务器或翻译 LSP 消息，具体语言服务器由 MCP 服务决定。调用前通过 `/mcp` 或 `mcp` `action="list_tools"` 发现实际工具名。`request_timeout_secs` 是独立 MCP 请求上限，长语言服务器操作可按需增大。旧技能 `required_tools: [lsp]` 迁到 `SKILL.md` 后应使用 `metadata.ax.required-tools: "mcp"`，通过 MCP 网关路由。

<a id="lazy-connection--capability-catalog"></a>
## 惰性连接与能力目录

- 启动时**不连接**服务，仅显式 `/mcp tools <server>` 或代理工具调用时连接、启动进程。
- `McpGateway` 以单个 `mcp` 工具，通过 `action="catalog"` 展示服务名、说明和声明能力，无需连接；`action="list_tools"` / `action="call"` 建立真实连接。
- `McpManager` 记录发现和调用失败，成功后清除错误并缓存发现元数据。disabled / sleeping / connected / error 状态反映实际配置及运行观测。

<a id="tool-proxies"></a>
## 工具代理

`McpToolProxy` 将远程 schema 转为普通 `Tool`，Agent Loop 无需 MCP 分支。动态名称 `mcp__server__tool` 避免跨服务冲突。显式发现成功后向工具注册表安装代理；断开／重启使代理运行时失效，下一请求获取更新工具。

<a id="mcp-command"></a>
## `/mcp` 命令

查看服务、运行状态及已发现工具：

- 输入搜索服务名、状态或传输。
- Enter 打开状态、配置协议、最后错误、缓存工具描述和 schema。
- **Alt+C** 连接并发现工具。
- **Alt+X** 断开并清除工具／错误缓存，后续使用可惰性重连。
- **Alt+R** 断开、重连并重新发现工具。
- Esc 返回。

连接失败在对话和管理器显示，UI 保持打开。`/mcp` 不编辑配置，不实现 MCP OAuth。清单／状态交互参考 Codex，但未引入其服务端框架。

<a id="reference"></a>
## 参考

| 职责 | 代码 |
|---|---|
| 配置解析 | `crates/mcp/src/config.rs` |
| 客户端与传输边界 | `crates/mcp/src/client.rs`、`crates/mcp/src/transport/`（`stdio.rs`、`http.rs`、`websocket.rs`） |
| 管理器、状态、缓存 | `crates/mcp/src/manager.rs` |
| 网关能力目录 | `crates/mcp/src/gateway.rs` |
| 工具注册表桥接 | `crates/mcp/src/bridge.rs` |
| `/mcp` UI | `crates/cli/src/tui/commands/catalogs.rs` |

<a id="importing-server-configuration"></a>
## 导入服务配置

`ax mcp import servers.toml` 验证并合并到项目 `.ax/mcp.toml` 或 `--mcp-config`；加 `--global` 使用 `$AX_HOME/mcp.toml`。支持含 `[mcp_servers.*]` 的 Codex `~/.codex/config.toml`，忽略无关模型设置，将 `http_headers`、`tool_timeout_sec` 映射到 AX 字段。环境引用凭据字段被拒绝，不静默丢弃认证。支持 Cursor（`~/.cursor/mcp.json`、`.cursor/mcp.json`）和 Claude Code（`~/.claude.json`、`.mcp.json`）中含 `servers` 或常见 `mcpServers` 对象的 JSON：command 项转 stdio，URL 项转 HTTP。支持显式 stdio、HTTP/streamable-http、WebSocket，拒绝旧 SSE。名称冲突拒绝整次导入，保留已有配置。以私有文件权限原子写入，不启动或连接服务。新会话才加载新配置。Crew 设置 → AX 能力使用相同 AX 命令。

<a id="local-execution-boundary"></a>
## 本地执行边界

本地 stdio 经 SandboxManager 执行，传输生命周期内保留管理器。配置 cwd 不能扩大绑定工作区，Skill 触发 MCP 也用同一传输。远程 MCP 有自身执行边界，远程副作用不在本地操作系统沙盒内。见[安全](security.zh-CN.md)。
