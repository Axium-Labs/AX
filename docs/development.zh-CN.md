<a id="development"></a>
# 开发

[English](development.md) | **简体中文**

本文介绍 AX 构建、测试及扩展。整体关系见[架构](architecture.zh-CN.md)，文档导航见 [docs/README.md](README.zh-CN.md)。

<a id="prerequisites"></a>
## 前提

- Rust **1.92+**，工作区 `rust-version`，edition 2024。
- 核心 AX 二进制不需要 Node 或 Docker；独立 Browser Use 另需 Node 和 Playwright，见[主机权限](host-permissions.zh-CN.md)。Release CI 使用原生 runner，为 Linux musl target 安装 `musl-tools`，见[发布](#releasing)。

<a id="workspace-layout"></a>
## 工作区布局

```text
Cargo.toml               # workspace root: 11 crates, shared deps, lints, release profile
crates/
├── cli/                 # composition root: args, provider selection, TUI, sessions
├── core/                # runtime-core: agent loop, context, budget, compression
├── evolution/           # low-frequency learning of owned Skills and Project facts
├── lexical/             # shared language-independent lexical features + similarity
├── memory/              # SQLite + JSONL storage, scoped facts, resume
├── mcp/                 # MCP client, transports, gateway, proxies
├── model/               # provider abstraction, auth storage, catalogs
├── sandbox/             # workspace confinement policies and OS execution backends
├── scoped/              # shared capability registry and scope resolution
├── skill/               # skill metadata indexing and routing
└── tool/                # Tool trait, built-ins, permissions, telemetry
```

<a id="build--test"></a>
## 构建与测试

```bash
cargo build --release        # binary at target/release/ax(.exe)
cargo test --workspace       # all unit tests
cargo test --package memory  # one crate
cargo clippy --workspace --all-targets   # lint (all + pedantic warnings)
cargo fmt --all              # formatting
```

本地构建的快速 smoke test：

私有回归测试不作为正式源码依赖。生产模块和 Cargo target 不得引用未跟踪本地测试文件。拟发布生产文件集需独立于本地测试覆盖验证；干净克隆不依赖私有 fixture 即可编译测试及执行静态检查。

```bash
./target/release/ax --version
./target/release/ax run "hello" --provider <configured-provider>
```

<a id="conventions"></a>
## 约定

Web 调度基准及可选公开提供商 smoke test 见 [web-latency.md](web-latency.zh-CN.md)。

- **单向依赖**：`cli` 负责组合，核心 crate 不依赖终端 UI，新模块放在边界正确一侧。
- **默认惰性**：提供商、技能、MCP、记忆按需加载，不把工作移到启动路径。
- **权限声明，不靠猜测**：各工具返回 `ToolPermission { capability, safety }`，不从工具名字符串推断。
- **上下文空间来自 `ContextBudget`**：不新增魔法字符数或固定窗口比例，见[上下文](context.zh-CN.md)。
- **绝不删除原始历史**：压缩只改模型输入，存储变更保持 JSONL 事件流权威。
- **Lint**：禁止 `unsafe_code`，clippy `all` + `pedantic` 为警告。

<a id="how-to-add"></a>
## 如何扩展

<a id="a-new-model"></a>
### 新模型

1. 新协议在 `crates/model` 实现 `ModelProvider`，参考 `openai_compatible.rs` / `openai.rs`。已有 Chat Completions 协议复用 `OpenAiCompatibleProvider`。
2. 在提供商目录注册 id／endpoint／env-var 和协议。
3. 在 `crates/cli/src/model_selection.rs` 接线解析，在 TUI 加登录选项。

<a id="a-new-built-in-tool"></a>
### 新内置工具

1. 在 `crates/tool` 实现 `Tool`：名称、说明、JSON 输入 schema、`permission()`，并经 `resources()` 显式声明读写资源。未知影响采用全局独占资源租约，声明是安全并行前提。见[并发](tools.zh-CN.md#concurrency)。
2. 支持隔离子执行时实现 `fork_for_run`，将工作区／会话／记忆／进程状态绑定到给定 `RunContext`，否则不进入子注册表。
3. 注册到组合根注册表，即 `crates/cli/src/runtime/builder.rs` 中 `runtime::tools`。

<a id="a-new-mcp-transport"></a>
### 新 MCP 传输

1. 在 `crates/mcp/src/transport/` 实现传输请求边界。
2. 在配置解析与客户端构造器增加传输类型。

<a id="a-new-skill"></a>
### 新技能

1. 创建 `skills/<name>/SKILL.md`，含标准 YAML frontmatter（`name`、`description`，可选 `license`、`compatibility`、`metadata`、`allowed-tools`）及 Markdown 正文。`description` 使用用户实际输入的词汇和语言描述任务与使用场景。路由基于词法相似度，若描述与请求毫无共同词汇，只能经元数据目录到达模型。验证与优先级见[技能](skills.zh-CN.md)。旧 `skill.toml` 加 `instructions.md` 仅用于已有用户数据兼容。
2. 无需代码变更，自动索引路由。

<a id="a-new-ui"></a>
### 新 UI

1. 消费 `AgentEvent` 并调用内核，参考 `crates/cli/src/tui/` 中现有 ratatui TUI。

<a id="storage-changes"></a>
### 存储变更

1. 会话／消息仓储语义保留在 `crates/memory`。
2. 用 `PRAGMA user_version` 保护**增量**迁移，绝不丢弃原始事件。见[存储](storage.zh-CN.md)。

<a id="releasing"></a>
## 发布

发布由 git tag 驱动：向 GitHub 推 `v*` tag 触发 `Release` 工作流（`.github/workflows/release.yml`）。

```bash
git tag -a v0.2.0 -m "AX 0.2.0"
git push origin v0.2.0
```

工作流：

1. 构建 Windows x64、Linux x64 GNU／musl、Linux ARM64 musl 和 macOS Intel/Apple Silicon 二进制。
2. 打包 Windows ZIP 及 Unix tar.gz，含可执行文件、`skills/`、`mcp.example.toml`，生成 `SHA256SUMS` 并发布 GitHub Release。发布工作流不运行测试。

原生平台构建／lint／测试由 `.github/workflows/platform-check.yml` 在 PR 或手动触发时运行。target、shell 默认及能力限制见[平台](platforms.zh-CN.md)。

安装器（`scripts/install.sh`、`scripts/install.ps1`）从 GitHub 取最新发布并用 `SHA256SUMS` 验证归档，将内置 `skills/` 放入 `$AX_HOME/skills`，跳过已有包；仅无配置时将 `mcp.example.toml` 复制到 `$AX_HOME/mcp.toml`。显式 `ax --update` 使用相同发布及校验，更新正在运行可执行文件所在路径，并刷新同一附带资源。Windows 的脱离进程 helper 等待 AX 退出后替换已验证二进制。更新代码位于 `crates/cli/src/update.rs`，普通启动不执行。其他用户／项目数据目录不动。

GitHub Releases API 不可用或限流时，`ax --update` 从官方 Release 页面解析 tag，并从该精确 tag 下载归档及 `SHA256SUMS`。临时下载失败最多重试两次，错误指出下载 URL 及需检查的网络／代理设置。Windows 同时使用系统代理和代理环境变量。旧 AX 更新器遇网络错误时，AX Crew 可回退官方安装器。Windows 安装器先重命名旧可执行文件，再复制已验证替代文件，复制失败则恢复；仍被运行任务使用的备份保留至可删除。

<a id="distributed-collaboration-validation"></a>
## 分布式协作验证

已公开 worker 回归测试通过 `cargo test --workspace` 覆盖 Git／快照隔离及传输策略。维护者集成验证还用真实 AX ACP worker 和 Crew gateway 配合受控提供商，覆盖租约到期、隔离旧所有者的接管及累计 Goal 预算恢复。本地集成 fixture 和输出不属于公开构建契约。

tag 驱动工作流仅在所有构建成功后一起发布所有桌面平台归档。Linux musl 资产名匹配 Unix 安装器和 `ax --update`，Linux x64 GNU 归档也继续提供。
