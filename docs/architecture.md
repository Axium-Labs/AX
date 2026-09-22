# ax workspace architecture

## 设计边界

`ax` 是 Agent Runtime Kernel，而不是聊天人格层。Kernel 只负责上下文、模型、工具、事件、权限和执行循环；Skill 与调用方决定任务行为。核心依赖保持单向，模块可以独立替换。

```text
cli ───────────────┬──> runtime-core ──> model
                   │         └─────────> tool
                   ├──> skill
                   ├──> memory
                   └──> mcp ───────────> tool (proxy implementation)
```

## Crate 职责

### `model`

定义 provider-neutral 的 `ModelProvider`、消息、Tool Call 和响应类型。`complete_stream` 是统一流式入口，默认可回退到非流式实现。现有 provider：

- DeepSeek Chat Completions；
- OpenAI Responses API；
- Codex 本地文件认证 + ChatGPT Codex Responses 端点。

Provider 同时报告 context window，供 kernel 的压缩策略使用。新增本地模型只需实现 trait，不需要修改 Agent Loop。

### `tool`

定义 `Tool`、`ToolRegistry`、JSON Schema、`SafetyLevel`，并提供 `shell` 与 `filesystem`。Kernel 在执行前统一查询 `ApprovalPolicy`；Tool 本身不依赖模型和会话。

### `runtime-core`

拥有：

- 最大步数受限的 model → tool → model Agent Loop；
- `AgentEvent` 流，包括 token delta、Tool 状态和压缩事件；
- Context 管理和基于模型容量的摘要压缩；
- `AgentSupervisor`，以独立上下文和有界并发运行任务。

核心没有固定 system prompt。只有执行上下文压缩时使用目标明确、无人格的总结指令。

### `mcp`

只读取 server 配置元数据，直到明确发现其工具才连接。实现 stdio、Streamable HTTP、WebSocket（扩展传输），支持 initialize 协商、现代无状态协议、分页 `tools/list`、`tools/call`、请求超时和进程清理。

`McpToolProxy` 把远端 schema 转成普通 `Tool`，因此 Agent Loop 不需要 MCP 分支。动态名称为 `mcp__server__tool`，避免跨服务器冲突。

### `skill`

首次使用时只索引 `skill.toml` 的 name、description、trigger keywords、required tools。路由命中且依赖满足后才读取 `instructions.md`。目录包格式天然可以由未来 Marketplace 下载和安装。

### `memory`

使用 bundled SQLite 和迁移版本管理，包含：

- `sessions`：标题、创建/更新时间和消息计数；
- `messages`：普通消息、Tool/MCP 调用和 Agent 状态；
- `session_summaries`：被压缩的旧上下文；
- `long_term_memory`：按 key/category 保存偏好、项目和决策。

读取采用 session scope 和稳定分页；不会启动时加载全部历史，也没有向量数据库、Embedding 或 RAG。

### `cli`

唯一 composition root。负责 clap 参数、provider 选择、SQLite/Skill/MCP 懒初始化、REPL、ratatui TUI、session 命令和权限交互。其他 crate 不依赖终端界面。

## 冷启动路径

```text
parse args
   ↓
construct lightweight ReplState
   ↓
show CLI / recent session metadata
   ↓ first task or explicit command
open selected session / build provider / index skills / connect one MCP server
```

具体保证：

1. provider 到第一次模型调用才构建；
2. Skill 到首次路由或 `/skills` 才索引 metadata；
3. `instructions.md` 到路由命中才读取；
4. MCP 到 `/mcp tools <server>` 才连接单个 server；
5. Memory 只恢复当前 session 的摘要和最近 200 条持久化消息；
6. SQLite 迁移只创建小型基础表和必要索引。

## Agent Loop

```text
user task
  → lazy Skill routing
  → context threshold check / optional summary
  → model streaming request
  → final text ──────────────→ persist and finish
  → tool calls
      → permission check
      → execute built-in or MCP proxy
      → append tool result
      → next model step
```

循环默认最多 12 个 model step，防止失控。所有 user、assistant、Tool、MCP 和 Skill 状态消息均写入当前 session。压缩会原样保留 Skill 等 system context，只用摘要替换较旧的对话与执行记录。

## 扩展点

- 新模型：实现 `ModelProvider`；
- 新内置工具：实现 `Tool` 并注册；
- 新 MCP transport：实现 transport request boundary；
- 新 Skill：添加 metadata 与 instructions 包；
- 新 UI：消费 `AgentEvent` 并调用 kernel；
- 新存储：保持 session/message repository 语义，避免把数据库泄漏到 core。
