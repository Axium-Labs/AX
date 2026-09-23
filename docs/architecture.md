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

凭据持久化在 `~/.ax/auth.json`：Unix 上写入后设为 `0600`；Windows 没有 mode bit，改为调用 `icacls` 移除继承的 ACE 并仅授予当前用户完全控制（尽力而为，工具不可用时不影响凭据保存本身）。

### `tool`

定义 `Tool`、`ToolRegistry`、JSON Schema、`SafetyLevel`，并提供 `shell`、`filesystem`、结构化的 `patch`（多段编辑，任意一段失败则整体不写入）与 `search`（行号限定的文本检索），减少对 shell 的滥用。

每个 Tool 通过 `permission()` 主动声明自己的 `ToolPermission { capability, safety }`，Kernel 与 UI 都不会从工具名字符串（例如 `mcp__`/`::`）猜测权限。`PermissionStore`（`tool::permission`）是唯一的权限存储：UI 与 Runtime 持有同一个 `Arc<RwLock<..>>` 的克隆，任何一侧修改策略都会立即对另一侧生效；此外支持会话级临时放行（`allow_session`），新会话开始时自动清空，且不能覆盖显式的 Deny。`telemetry` 子模块提供进程内、无敏感数据的延迟指标（`Timer`/`snapshot`），供 `/status` 面板展示。

### `runtime-core`

拥有：

- 受 `ExecutionBudget`（最大 model step 数、最大 Tool 调用数、单轮超时、单次 Tool 超时，均可通过 CLI flag 配置）约束的 model → tool → model Agent Loop；预算或超时触发时，会为悬挂的 Tool Call 补上占位结果，保持消息记录合法；
- `AgentEvent` 流，包括 token delta、Tool 状态和压缩事件；
- Context 管理：`context::select_context` 按模型 token 预算（而非固定消息条数）挑选恢复到上下文的历史消息，并保证不切断未完成的 Tool 调用轮次；
- `ContextBudget`（`runtime-core::budget`）：把 context window 拆成真正可用的空间，每一层都是显式命名的方法/常量：回复预留（`RESERVED_OUTPUT_TOKENS`）和当前 Tool schema 估算 token 数从 context window 中减去得到 `usable()`；再从中为 Skill 说明和检索到的记忆各保留一部分（`SKILLS_RESERVE_TOKENS`/`MEMORY_RESERVE_TOKENS`）得到 `history_budget()`；`history_budget()` 又按 `SESSION_SUMMARY_SHARE_PERCENT` 拆分为 `session_summary_budget()`（恢复 session 时的持久化摘要/系统状态）和 `recent_messages_budget()`（逐字保留的最近对话），防止过大的 summary 独占整个预算而挤掉最近消息；`compact_threshold()` 则直接对 `usable()` 取百分比。所有和上下文空间相关的限制（max output、Tool schema、system/runtime context、memory、skills、session summary、recent messages）都从这一个结构推导，不再各模块各自硬编码固定字符数或对原始 context window 取固定比例；
- 基于模型容量的摘要压缩：压缩只影响“喂给模型的上下文”，压缩阈值按 `ContextBudget` 计算的可用空间而非原始 context window 取比例；只在 `session_summaries`（每 session 一行）upsert 最新累积摘要并推进覆盖水位线，从不删除 `messages` 表中的历史原文；
- `AgentSupervisor`，以独立上下文和有界并发运行任务。

核心没有固定 system prompt。只有执行上下文压缩时使用目标明确、无人格的总结指令。

### `mcp`

只读取 server 配置元数据，直到明确发现其工具才连接。实现 stdio、Streamable HTTP、WebSocket（扩展传输），支持 initialize 协商、现代无状态协议、分页 `tools/list`、`tools/call`、请求超时和进程清理。

`McpToolProxy` 把远端 schema 转成普通 `Tool`，因此 Agent Loop 不需要 MCP 分支。动态名称为 `mcp__server__tool`，避免跨服务器冲突。`McpGateway`（单一 `mcp` Tool）额外暴露一个轻量 capability catalog：即使 server 从未连接，模型也能通过 `action="catalog"` 看到其名称、描述和声明的 capabilities，只有 `list_tools`/`call` 才会真正建立连接。

### `skill`

首次使用时只索引 `skill.toml` 的 name、description、trigger keywords、required tools。路由不再只取 top-1 关键词命中项，而是返回所有依赖工具齐备的候选（`route_candidates`），交给调用方按需选择；命中且依赖满足后才读取对应 `instructions.md`。目录包格式天然可以由未来 Marketplace 下载和安装。

### `memory`

SQLite stores raw session messages, effective context snapshots, and scoped facts. Global facts live in the AX home; Project and Session facts live in the selected project database. Project ownership uses a UUID persisted in `.ax/project-id`, independent of the absolute workspace path and `--data-dir`.

Explicit `remember key=value` declarations default to Session. Natural language intent is handled by the main model through a session-bound `memory` tool; code validates scope ownership, quoted user provenance, credential patterns, and optimistic update preconditions. Retrieval uses lexical relevance, update time, explicit Global inclusion settings, and the shared token budget.

Each complete conversation message is checkpointed before execution advances. Resume queries pages after the snapshot watermark and marks interrupted tool calls without replaying their side effects. Compression snapshots remain session-local; raw messages are retained. See [Memory behavior](memory.md) for migration, UI controls, and current limits.

### `cli`

唯一 composition root。负责 clap 参数、provider 选择、SQLite/Skill/MCP 懒初始化、REPL、ratatui TUI、session 命令和权限交互。其他 crate 不依赖终端界面。

`cli::providers` 是“哪些 provider 已配置”的唯一定义（AX 自己的 `auth.json`、约定环境变量、显式传入的旧版 Codex auth 路径），CLI 启动解析（`model_selection`）和 TUI 的 `/model` 目录刷新（`tui::catalog_refresh`）都委托给它，不再各自实现一份容易漂移的判断逻辑（之前的问题：只设环境变量时，CLI 启动会自动选中该 provider，但 `/model` 却显示它未配置）。

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
4. MCP 的进程/连接到 `/mcp tools <server>` 才建立，但其 capability catalog（服务器名称/描述/声明能力）无需连接即对模型可见；
5. Memory 只恢复当前 session 的摘要，历史消息按模型 token 预算选取（而非固定条数），历史原文在 SQLite 中完整保留；
6. SQLite 迁移只创建小型基础表和必要索引。

## Agent Loop

```text
user task
  → lazy Skill routing
  → retrieve memory and checkpoint user input
  → context pressure check / layered compression
  → model streaming request
  → final text ──────────────→ persist and finish
  → tool calls
      → permission check
      → execute built-in or MCP proxy
      → append tool result
      → checkpoint result
      → context pressure check / next model step
```

循环受 `ExecutionBudget` 约束（默认最多 64 个 model step、128 次 Tool 调用、600s 单轮超时、120s 单次 Tool 超时，均可用 `--max-steps`/`--max-tool-calls`/`--turn-timeout-secs`/`--tool-timeout-secs` 覆盖），防止失控。预算用尽或超时时会为任何悬挂的 Tool Call 补上中断提示，保持消息记录合法。所有 user、assistant、Tool、MCP 和 Skill 状态消息均写入当前 session。压缩会原样保留 Skill 等 system context与完整历史原文，只用摘要替换送给模型的较旧对话与执行记录。每个 model step 和 Tool 调用都会记录延迟指标，可通过 `/status` 查看。

## 扩展点

- 新模型：实现 `ModelProvider`；
- 新内置工具：实现 `Tool` 并注册；
- 新 MCP transport：实现 transport request boundary；
- 新 Skill：添加 metadata 与 instructions 包；
- 新 UI：消费 `AgentEvent` 并调用 kernel；
- 新存储：保持 session/message repository 语义，避免把数据库泄漏到 core。
