# ax

`ax` 是一个轻量、Rust Native 的 Agent Runtime Kernel。它不是绑定固定人格的聊天机器人；运行时行为由当前上下文、按需加载的 Skill、用户任务和可用 Tool 动态组成。

## 已实现能力

- Rust workspace，crate 目录不加 `ax-` 前缀，最终二进制名为 `ax`；
- 有界 Agent Loop、流式输出、运行时事件和危险操作权限层；
- DeepSeek Chat Completions、OpenAI Responses API、AX 自有 Codex OAuth；
- 统一 Tool Registry，内置 `shell`、`filesystem`，以及 MCP Tool 动态代理；
- MCP stdio、Streamable HTTP 和可选 WebSocket 传输，按服务器懒连接；
- metadata-first Skill 索引、关键词路由和 `instructions.md` 懒加载；
- SQLite Session、消息历史、长期记忆和自动上下文摘要压缩；
- 普通交互 CLI、保留终端 scrollback 的 inline `ratatui` TUI、命令补全；
- 有界并发的隔离上下文多 Agent 调度。

## Workspace

```text
ax/
├── crates/
│   ├── cli/       # ax 入口、REPL、TUI、模块装配
│   ├── core/      # Agent Loop、Context、Event、压缩、多 Agent
│   ├── model/     # ModelProvider、DeepSeek、OpenAI/Codex
│   ├── tool/      # Tool、Registry、权限、shell/filesystem
│   ├── mcp/       # MCP client、传输、管理器、Tool bridge
│   ├── skill/     # metadata 索引、路由、指令懒加载
│   └── memory/    # SQLite Session、History、Long-term Memory
├── skills/
├── mcp.example.toml
└── docs/architecture.md
```

## 快速开始

需要 Rust 1.92+。

```powershell
$env:DEEPSEEK_API_KEY = "..."
cargo run -p cli
```

一次性任务、TUI 和多 Agent：

```powershell
cargo run -p cli -- run "列出当前项目结构"
cargo run -p cli -- tui
cargo run -p cli -- agents "检查架构" "检查错误处理" --concurrency 2
```

默认 provider 是 DeepSeek。其他 provider：

```powershell
$env:OPENAI_API_KEY = "..."
cargo run -p cli -- --provider openai --model gpt-5.3-codex

# 先在 TUI 中执行 /login，选择 OpenAI Codex
cargo run -p cli -- --provider codex
```

`/login` 得到的 API key 和 Codex OAuth 均保存在 AX 自己的 `~/.ax/auth.json`（设置 `AX_HOME` 时为 `$AX_HOME/auth.json`），不会默认读取或修改 `~/.codex/auth.json`。`--codex-auth <path>` 仅作为显式的旧版兼容入口。可通过 `OPENAI_API_URL`、`CODEX_API_URL` 或 `DEEPSEEK_API_URL` 覆盖端点；`--context-window <tokens>` 可覆盖 provider 默认容量，使压缩阈值与实际部署一致。

## 会话与命令

状态默认保存到 `.ax/memory.sqlite3`，可用 `--data-dir <path>` 修改。启动时不会载入全部历史；`/resume` 只恢复目标 session 的摘要和最近消息。输入 `/` 会立即打开统一命令菜单。

```text
/login
/logout
/model
/resume
/new
/memory
/compact
/skills
/tools
/mcp
/permissions
/status
/exit
```

`/login` 沿用 pi 的两级认证选择：先选择 Account/OAuth 或 API Key，再选择 provider；`/logout` 只显示并删除 AX 自己保存的凭据，不修改环境变量。完整 provider 接口仍然保留。

`/model` 采用与 pi 相同的“可用模型快照”思路：先从 `~/.ax/models` 立即显示上次成功的缓存，再在后台仅为已登录 provider 调用实时目录。DeepSeek、OpenAI-compatible 厂商使用其认证后的 `/models`，Codex 使用账户 catalog；实时成功后按 provider 原子替换缓存。未登录、静态 models.dev 清单和源码硬编码模型都不会进入选择器，因此不会因为旧目录展示已经不可用的模型。

普通 REPL 和 TUI 会逐次确认危险工具；一次性和多 Agent 模式默认拒绝危险工具。显式传入 `--allow-dangerous` 才会放行。

## Skill

Skill 位于 `skills/<name>/`：

```text
skills/coding/
├── skill.toml
└── instructions.md
```

启动时不扫描 Skill。首次路由或执行 `/skills` 时只读取 `skill.toml`；关键词匹配且 required tools 均可用后，才读取 `instructions.md` 并注入当前 session。可用 `--skills-dir <path>` 修改根目录。

## MCP

把 `mcp.example.toml` 复制到 `.ax/mcp.toml`，或传入 `--mcp-config <path>`。`/mcp` 只查看元数据，不连接服务器；`/mcp tools <server>` 才建立该服务器连接、分页发现 Tool，并将其注册为 `mcp__<server>__<tool>`。远程 Tool 默认按危险操作处理，只有声明 `readOnlyHint = true` 的 Tool 视为安全。

## Memory Compression

Provider 暴露当前模型的 context window。上下文估算达到默认 75% 时，kernel 调用当前模型总结旧消息，保留用户目标、已完成工作、代码修改、Tool 结果、约束、决策和未完成任务，再将摘要与最近 12 条消息原样保留。SQLite 同步保存摘要并删除已被替代的旧消息，不做简单截断，也不依赖向量数据库、Embedding 或 RAG。

## 验证

```powershell
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

更详细的依赖方向和启动路径见 [docs/architecture.md](docs/architecture.md)。
