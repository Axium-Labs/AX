<a id="ax-architecture"></a>
# AX 架构

[English](architecture.md) | **简体中文**

本文说明整体架构：crate 布局、Agent Loop 和冷启动路径。专题细节见各自文档，完整索引见 [docs/README.md](README.zh-CN.md)。

<a id="design-boundaries"></a>
## 设计边界

AX 是 **Agent 运行时内核**，不是聊天人格层。内核负责上下文、模型、工具、事件、权限和执行循环，技能及调用方决定任务行为。核心依赖单向，模块可独立替换。

```text
cli ───────────────┬──> runtime-core ──> model
                   │         └─────────> tool
                   ├──> skill ────┐
                   ├──> memory ───┴──> lexical
                   └──> mcp ───────────> tool (proxy implementation)
```

<a id="crate-responsibilities"></a>
## Crate 职责

| Crate | 职责 |
|---|---|
| `model` | 提供商中立 `ModelProvider`、文本／图像消息、能力、工具调用和响应。统一流入口 `complete_stream`，支持非流式回退。 |
| `tool` | `Tool`、`ToolRegistry`、JSON Schema、`SafetyLevel`，及内置 `shell`、`filesystem`、`find_files`/`glob`、`patch`、`search`、`web`、`view_image`、`browser`、原生 `desktop`。桌面使用 Windows UIA、Linux AT-SPI、macOS Accessibility，默认开启，需独立应用访问及平台前提。拥有 `PermissionStore`。 |
| `runtime-core` | 模型→工具→模型循环、`AgentEvent`、上下文选择、`ContextBudget`、压缩及有界并发 `AgentSupervisor`。 |
| `mcp` | stdio／Streamable HTTP／WebSocket 客户端、惰性连接、能力目录、`McpToolProxy`、`McpGateway`。 |
| `skill` | `SKILL.md` frontmatter 索引、预计算 Unicode 特征、语言无关相似度路由；命中路由才加载正文，兼容旧包。 |
| `scoped` | Global/Project 共用注册表、配置策略及覆盖／屏蔽解析。仅元数据，不依赖执行或 UI。 |
| `sandbox` | 工具及本地 MCP 下方的运行时限制策略、可复用 OS 后端与工作区管理器。 |
| `lexical` | 语言无关 NFKC、大小写折叠、词 token、字符 n-gram 及规范相似度，技能／记忆共用避免漂移。叶子 crate，不依赖其他 AX crate。 |
| `memory` | SQLite 会话／消息仓储、有效上下文快照、作用域事实、JSONL 流、恢复和压缩状态。 |
| `cli` | 唯一组合根：clap、模型选择、惰性 SQLite/Skill/MCP、REPL、ratatui TUI、会话和权限对话框。 |
| `evolution` | CLI 输入的有界 Experience、低频进程内分析、所属标准 Skill 生命周期和学习 Project 事实，不依赖 core/tool。 |

显式 `ax --update` 属 CLI，在普通初始化前退出。检查最新 GitHub Release，用 `SHA256SUMS` 验证并替换当前程序；Windows 等运行进程退出再替换。刷新内置技能和 MCP 示例，不覆盖用户配置，详见[发布](development.zh-CN.md#releasing)。

<a id="local-system-resources"></a>
### 本地系统资源

CLI `/system` 用 `sysinfo` 按需收集 CPU 和物理 RAM，CPU 在库要求的最小间隔两次刷新间计算。斜杠命令等待阻塞线程池，不在普通启动监控或加载 GPU 驱动。每次重开重新取快照。

可选 `nvml-wrapper` 惰性加载已安装 NVIDIA NVML 并保留句柄。各支持 GPU 报名称、利用率、专用显存已用／总量，单查询失败只让对应字段不可用。缺驱动或 Intel/AMD 等不支持厂商显示遥测不可用，不虚构容量或断言无 GPU。RAM/VRAM 用 GiB，内部 `_mb` 为 MiB。此 CLI 模块不提供 AXCrew 独立系统设置采集或 worker 硬件清单。

<a id="model"></a>
### `model`

定义提供商中立 `ModelProvider`、消息、工具调用和响应。`complete_stream` 为统一流入口，默认回退非流实现。现有协议适配器：

- OpenAI-compatible Chat Completions，涵盖 DeepSeek、Groq、Mistral、OpenRouter、Together、xAI、Kimi 等；
- 原生 WorkBuddy 浏览器授权、轮换 OAuth 和直接 Chat Completions，见[提供商](providers.zh-CN.md#native-workbuddy)；
- OpenAI Responses API；
- Codex 本地文件认证与 ChatGPT Codex Responses。

`ProviderSpec` 保存厂商身份、凭据环境变量及协议选择。`OpenAiCompatibleProvider` 实现共享格式，DeepSeek 端点及默认仍在元数据。提供商报告上下文窗口，驱动压缩；本地模型只需实现 trait，无需改循环。

凭据持久化于 `<install-dir>/.ax/auth.json` 或 `AX_HOME`。Unix 权限 `0600`；Windows 无该模式位，调用 `icacls` 移除继承 ACE 并给当前用户完全控制，尽力执行，保存不依赖其成功。

<a id="tool"></a>
### `tool`

定义 `Tool`、`ToolRegistry`、JSON Schema、`SafetyLevel`，提供 `shell`、`filesystem`、`find_files`/`glob` 名称／路径／扩展发现、结构化 `patch` 多块编辑和 `search` 行内内容／符号／正则搜索，减少滥用 shell。任一补丁块失败整次写入终止。发现共用 `crates/tool/src/discovery.rs` 遍历与 glob 策略，相同根的文件名／内容扫描不分歧。

各工具经 `permission()` 返回 `ToolPermission { capability, safety }`，内核／UI 不从如 `mcp__` / `::` 的名字猜权限。`PermissionStore`（`tool::permission`）是唯一来源，UI／运行时克隆同一 `Arc<RwLock<..>>`，变更立即互通。`allow_session` 临时允许在会话开始清除，不能覆盖显式 Deny。`telemetry` 提供进程内无敏感信息延迟 `Timer`/`snapshot`，供 `/status`。

内核构建、工具插入和子模型覆盖时绑定所选模型到 WebTool。并发查询经 NativeFirst 先运行真实服务搜索，再经共享 SearchRouter。匿名 Exa/Parallel MCP 普通回退，各自健康、延迟、冷却、有界对冲和取消。结果保留来源／引用供后续生成。已知 URL fetch 保留独立传输／权限，见[工具](tools.zh-CN.md#web)及[完整审计](web-search-audit.zh-CN.md)。

<a id="runtime-core"></a>
### `runtime-core`

内部 `kernel/` 管状态／构造／Goal，`loop_runtime/` 管模型步、工具步及排序循环，`compression/` 管管道和结构摘要，`event` / `error` / `approval` / `token` 为对外契约，`budget` / `context` / `scheduler` / `execution` / `child` / `child_policy` / `subagent` / `supervisor` / `task_queue` 为执行机制。`lib.rs` 只含 `mod` 和 `pub use` 门面。

负责：

- **Agent Loop** 与 `ExecutionBudget`，含最大模型步、工具调用、轮次和工具超时，四者默认无限，正 CLI 值启有限。预算／超时触发时给挂起工具占位结果，保持历史格式。
- **`AgentEvent`**：Token 增量、工具状态、压缩事件。`SubagentActivity` 按执行身份隔离公开文字、工具和生命周期，嵌套含直接父链接。ACP 展示独立子信封，记录父所有恢复索引，原子历史留子存储，不含子推理，见 [ACP 委派活动](acp-crew.zh-CN.md#delegated-agent-activity)。
- **识别依赖的工具轮次**：独立声明副作用有界并发，类型化结果引用及共享写资源加 DAG 边。进程级租约跨内核串行冲突文件、Git、Memory。事件／结果留真实调用 ID；原检查点按完成顺序，下请求按调用顺序。见[并发](tools.zh-CN.md#concurrency)、[ADR 0005](adr/0005-tool-round-scheduler.zh-CN.md)。
- **上下文**：`context::select_context` 按 Token，不按固定消息数，且不截断未完成工具轮。
- **`ContextBudget`**：`runtime-core::ContextBudget` 在 `crates/core/src/budget.rs` 派生全部上下文限制，含输出预留、schema 估算、弹性池、各来源上限及下请求预计压缩。模块不硬编码字符数／原窗口比例，见[上下文](context.zh-CN.md)。
- **容量感知压缩**：只改模型输入，按 Session upsert 累计摘要、推进水位，不删 `messages` 原消息。见[记忆](memory.zh-CN.md)、[存储](storage.zh-CN.md)。
- **`AgentSupervisor`**：独立上下文和有界并发任务。

Core 无人格或分类提示。每执行注入 `[ax-agent-runtime]`、注册工具所有的 `[ax-capability-guidance]`、有界 `[ax-environment]`，含 cwd、根、沙盒。它们说明边界、环境和选中能力后的用法，不暗示任务或扩范围。压缩用聚焦非人格化指令；长任务只增加临时进度和恢复，见[运行时](agent-runtime.zh-CN.md)。

<a id="mcp"></a>
### `mcp`

工具需要前只读配置元数据。实现 stdio、Streamable HTTP、WebSocket、`initialize` 协商、现代无状态协议、分页 `tools/list`、`tools/call`、超时与清理。

`McpToolProxy` 将 schema 转普通 `Tool`，循环不需分支；动态名 `mcp__server__tool` 避免冲突。单 `mcp` 工具 `McpGateway` 用 `action="catalog"` 展示未连接服务名称、描述、声明能力，仅 `list_tools`/`call` 建真实连接。见 [mcp.md](mcp.zh-CN.md)。

<a id="skill"></a>
### `skill`

首次只索引 `SKILL.md` YAML 或旧 `skill.toml`，预计算词法特征，仅用于排序。主模型接紧凑合格元数据，显式 `invoke_skill`，正文／资源惰性。确定性启用、作用域、路径、依赖、隐式调用过滤限制选择，不增路由模型请求。见[技能](skills.zh-CN.md)。

<a id="memory"></a>
### `memory`

内部 `store.rs` 管打开、迁移、`MemoryStore`；`schema.rs` / `migrations.rs` 管增量 schema 和顺序；`session.rs` / `message.rs` / `context.rs` / `long_term.rs` 为四项存储；`events.rs` 管流和索引；另有 `types.rs` / `error.rs`、`scoped.rs`、`backup.rs`。`MemoryStore` 为单一门面。

SQLite 保存会话消息、快照、事实。Global 位于 AX 主目录，Project/Session 位于选定安装项目库。UUID 存 `<install-dir>/.ax/projects/<project-key>/project.json`，独立 `--data-dir`，旧工作区 ID 兼容；状态跨工作区删除保留。迁移及路径键见[存储](storage.zh-CN.md)。

显式 `remember key=value` 默认 Session，主模型通过会话绑定 `memory` 处理自然语言，代码验证归属、用户原文、凭据及乐观前提。检索先筛当前 Session/Project 和 Global，最多 32 词法／精确／标签／路径候选，再按相关、使用、置信及类型时效轻排。作用域只过滤不计分。少量 SQLite 摘要进入共享预算，详情按键读，复用 Unicode 特征，无嵌入、向量库、语言专属规则或额外模型调用。

完整消息在推进前保存检查点。恢复分页读快照水位后，标记中断不重放副作用。压缩快照仅 Session，原消息保留，见[记忆](memory.zh-CN.md)、[存储](storage.zh-CN.md)。

<a id="cli"></a>
### `cli`

`main.rs` 解析／引导／分发／退出；`args.rs` 定义 clap，`bootstrap.rs` 管沙盒和状态位置，`app.rs` 路由，`repl/` 管会话，`commands/` 管 run/agents/导入导出，`runtime/` 组装内核／提供商／工具。其他模块各管命令族或前端。

唯一组合根，负责 clap、提供商选择、惰性 SQLite/Skill/MCP、REPL、ratatui TUI、会话及权限对话框。其他 crate 不依赖终端 UI。

`cli::providers` 唯一定义“已配置提供商”，来源为 AX `auth.json`、惯常环境变量及显式旧 Codex 路径。启动 `model_selection` 与 TUI `/model` 刷新 `tui::catalog_refresh` 都委托它，避免维护漂移副本。

<a id="cold-start-path"></a>
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

保证：

1. 首模型调用才构建提供商。
2. 首路由或 `/skills` 才索引元数据。
3. 路由命中才读技能正文。
4. `/mcp tools <server>` 才建进程／连接；名称、描述和声明能力无需连接可见。
5. Memory 只恢复当前摘要，历史按模型 Token 预算而非固定数量选择，SQLite/JSONL 原历史完整。
6. SQLite 迁移只建小基础表及必需索引。

<a id="agent-loop"></a>
## Agent Loop

```text
user task
  → compact eligible Skill metadata
  → retrieve memory and checkpoint user input
  → inject runtime prompt + per-capability guidance + environment snapshot + coding policy
  → context pressure check / layered compression
  → model streaming request
  → final + no continuation → optional Goal (default absent) → TurnFinished
  → pending input / approval / child / retry → wait, collect results, continue
  → tool calls
      → goal-bound step/scope admission
      → permission check
      → dependency DAG and bounded resource-aware execution
      → update ExecutionState from actual completion events
      → advisory loop-hygiene reminder on repeated identical calls
      → checkpoint completed results and execution state with original tool call IDs
      → append completed round in original call order
      → context pressure check / next model step
```

默认无步骤、调用、轮次或工具时限。`--max-steps`、`--max-tool-calls`、`--turn-timeout-secs`、`--tool-timeout-secs` 正值启用，`0` 无限。触发时挂起调用获中断占位。全部用户、助手、工具、MCP 和技能状态写当前会话。压缩保留技能系统上下文及完整原历史，仅将模型输入中的旧对话／执行记录替换摘要。各模型步和工具调用记录 `/status` 延迟。

完成共用 `TurnState` / `TurnContinuation`。默认最终文本立即流出，不经审查器。状态、配置及遥测见 [agent-loop.md](agent-loop.zh-CN.md)。

<a id="extension-points"></a>
## 扩展点

| 新增内容 | 修改位置 |
|---|---|
| 新模型 | 实现 `ModelProvider` |
| 新内置工具 | 实现并注册 `Tool` |
| MCP 传输 | 实现传输请求边界 |
| 技能 | 添加 `SKILL.md` 包 |
| UI | 消费 `AgentEvent` 并调用内核 |
| 存储 | 保持会话／消息语义，不向 core 泄露数据库 |
| 可移植备份 | 用 memory ExportService/ImportService，CLI 管参数和展示 |

步骤见[开发](development.zh-CN.md)。

<a id="lightweight-long-task-orchestration"></a>
## 轻量长任务编排

各控制器轮次有既有 Goal ID 和空活跃队列头，普通直接完成关闭头，无可选验证。具体条目来自 `task_queue` 或类型化工作观测。显式 Goal 附验收和生命周期到同头，普通身份本身不启 Goal。CLI 可经 Pursue 将普通新任务纳入启用的作用域模板，默认不自动使用。见[运行时](agent-runtime.zh-CN.md)、[Goal](goal.zh-CN.md)。

现有 Loop 和工具 DAG 继续执行。小队列用 `task_queue` 或 `ax_work_items` 存具体模型任务，编号／项目符号只是提示，不创建队列，无独立规划请求或固定编排系统提示。`task_queue start` 接完整字符串或 `{title, input}`，标题仅展示，子收到完整 `input` 和显式上下文，不收标题替代品或兄弟／控制器历史。可先读数据再动态建实例。活跃同 Goal 队列未分发任务可再显式 `start` 替换，旧计划归档 superseded。工具／子配置前先存分发标记，避免恢复时替换未知在途。已有子身份、结果或失败也阻止替换，已执行工作需新 Goal；旧字符串以完整标题作输入。

队列属于 `goal_id`，不是 Session。普通新轮启动新 Goal，取代归档可恢复旧队列并持久新头。取代、归档及新头在同检查点，避免半应用。`GoalTurn::Resume` 仅恢复明确 ID，预算中断暂停待显式恢复。CLI 提供 `ax run --session
<ID> --resume-goal <GOAL_ID> <PROMPT>` 和 `--cancel-goal`；ACP `_meta.axGoal` 的 `action` 为 new/start/resume/cancel，适用时传 `goal_id`，成功返回 `_meta.axGoal.goal_id`。

任务为 pending/running/completed/failed/skipped，显式 `finish` 推进，失败前恢复是编排选择。工具失败／超时是局部，独立工作可继续；恢复记录失败步骤、调用 ID、工具、路径资源作建议，不收窄或禁诊断、重试、后续项。提供商按分类的次数／时间预算重试；配置、认证、持久化及耗尽重试阻塞 Goal。

队列为 active/summarizing/suspended/completed/blocked/cancelled/superseded。显式全局 `block`/`cancel` 和 `ToolError::GlobalBlocked` 立即停止消费。纯文本不推进任务，未完成时继续，全部终态后完成摘要。终态响应输出前保存并缓存，重连无需模型或重复事件。Worker fork 去控制器队列，只保留父标识，不需人工新建会话。检查点用既有 AgentState，见[上下文](context.zh-CN.md)、[存储](storage.zh-CN.md)、[ADR 0008](adr/0008-durable-task-queue.zh-CN.md)。

<a id="automatic-isolated-child-execution"></a>
### 自动隔离子执行

CLI/TUI/ACP/Crew 共用 `run_prompt_with`；`child_runtime::configure_controller` 经单 `LocalChildHost::for_controller` 每提示绑定项目、存储排除、控制器及独立子预算，包括恢复／预初始化内核。ACP 早期分发经 `run_session_prompt` 到达此边界，不增 Loop。子共用内核和 `ToolRegistry`，按 RunContext 重绑，不收控制器队列／兄弟消息。普通单任务循环不变，嵌入方可 `with_child_host`。

模型显式 `execution="children"` 后，不再每模型轮一个任务，而分发整个**就绪前沿**。`pending` 在全部依赖 `Completed` 时才就绪；`resources` 不冲突的全部就绪项同批准备启动，受共用 `SubagentConfig.max_concurrent` 池限制，默认 8，`with_child_concurrency` 兼容同上限。槽从准备到回执收集，与普通子代理和后代共享。完成后立即接新就绪，不逐个等待。失败／超时留回执继续。取消／丢轮次一并丢所有子 future，不保留脱离任务。

类型化读取清单初始控制器模式，可用 `task_queue action=configure, execution=children` 切换，不替换条目。准入／分发含恢复都重检深度和工具限制，恢复后禁委派保留句柄／回执并反馈模型。队列子继承绝对深度／配置，低于上限才普通委派，队列控制仅父可用。

资源模型判断冲突，`resources` 可路径或名称，修改用 `write: true`，重叠访问不并发。工具冲突仍由既有进程租约串行，无新机制。

各子返回 `ChildResult`，含状态、摘要、发现、文件、diff、诊断、验证、产物、原因、续接和指标。父模型仅得 `model_summary()`，完整持久记录经 `child_result` 读取，恢复细节不重跑。全部终态后父进入摘要。旧回执短路准备，恢复不重跑已完成子。见 [ADR 0016](adr/0016-parallel-children-and-instructions.zh-CN.md)。

<a id="project-instructions-and-asking-the-user"></a>
### 项目指令与用户提问

`instructions::InstructionResolver` 确定性解析全局、根 `AGENTS.md`、根到 cwd 每级 `AGENTS.md`，同级 `AGENTS.override.md` 替代，再选 `path`/glob 匹配任务文件的 `.ax/rules/*.md`。段含 `source_path`、范围、优先和来源，最具体优先，以 `ContextBudget::instructions_budget_tokens()` 有界，始终启用链保留。不查记忆／嵌入／语义索引。

项目指令独立 `[ax-project-instructions]`，与记忆、技能、系统上下文不互替，每轮重新解析，恢复不陈旧。见[上下文](context.zh-CN.md#project-instructions)。

`request_user_input` 处理实质影响结果且仓库无法回答的问题，绝非授权门槛。提问保存 Goal、队列、Session、问题，挂 `waiting_for_user`，不失败、不丢弃。答案写回同调用 id，`GoalTurn::Answer` 从原位置继续，不新用户轮次。

`LocalChildHost` 将 `child-runs/<id>/state` 的数据库、JSONL、回执、清单与可丢 `workspace` 分开。Git 采用捕获 HEAD 的 detached worktree，加包含暂存／未暂存跟踪修改、重命名、删除的二进制脏补丁，只复制合格未跟踪输入，不递归复制未变跟踪文件。非 Git 或 Git 不可用用过滤快照。嵌套 `.gitignore`、`.axignore` 含否定规则参与排除，同时排控制器库、兄弟历史、符号链接、构建缓存。已有 Git 库 worktree／补丁出错只失败该子，不静默复制全项目。Runtime/scripts 和已装 PATH 保留。

完成／失败先回执再清工作区和 worktree 注册，清理失败 GC 重试，回执无工作区也能恢复。运行／中断保留待恢复，旧 `workspace/.ax` 迁 `state`。文件租约阻 GC，根准入锁覆盖状态目录、子租约、清单发布，防 GC 认领半发布；复制及执行仍并行。GC 在准入运行，删终态，按 idle TTL 过期无租约工作区，不删原历史。到期恢复明确失败，不静默重启。工作区变化可丢、不合并、非保留产物，持久输出／历史在 `state`。

工具经 `Tool::fork_for_run(RunContext)` 主动支持。文件／图像／搜索／补丁路径及租约重绑，shell 显式 cwd 和子 AX_HOME／会话／记忆环境；所有记忆作用域都用子库。未绑定扩展和父 MCP 不静默共享，扩展必须有子作用域绑定。原结果读器独立，事件 ID 加子 Session 前缀避 UI 冲突。

子失败、未解决错误、工作区失败或超时只记录该子并推进。嵌入方 `with_child_execution_budget` 设独立限制，CLI `--child-timeout-secs` 独立 `--turn-timeout-secs`。控制器 ExecutionBudget 是全局停／挂边界，取消／阻塞停止消费。原子历史和终回执在子 Session；恢复复用身份／cwd／历史，控制器检查点中断也不重放完成子。未知工具占位待检查，不自动重放；子最终文字不作为控制器最终输出。

配额默认每子 2 GiB、每根 8 GiB，idle TTL 七天。正整数环境覆盖 `AX_CHILD_WORKSPACE_QUOTA_BYTES`、`AX_CHILD_TOTAL_QUOTA_BYTES`、`AX_CHILD_WORKSPACE_TTL_SECS`。准入预估快照／checkout 大小及余量，每原始检查点核实际占用。超限仅失败该子，存失败并释放空间。这是准入／检查点限制，非 OS 配额，子进程中间可暂时超限。持久状态不计工作区额且 GC 保留。终态／到期回收；无可读清单的准备中断孤儿也按 TTL 回收，避免永久泄漏。

<a id="goal-bound-execution-invariants"></a>
### 绑定 Goal 的执行不变量

`ExecutionState` 独立于压缩，保留原目标／身份、当前步骤、预期输出、范围、失败恢复绑定、有界真实事件及进度。模型通过普通调用 `_ax_execution` 声明。步骤内可收窄，新步骤在任一观测后可选初始工作区内目录。类型化结果替换后、审批执行前按声明资源检查。空、无匹配、失败都使 `running`→`observed`，无需正进展即可重试／重规划／下一步／明确失败。

八次无声明转移触发建议性 `NoProgressDetector`。read/list/search 成功为观测，新成功变更、显式证据、修复重试计进度，重复去重。计数、恢复、重试／探索数不限制准入；权限、沙盒、冲突、依赖、预算／超时及取消仍强制。未知 shell/MCP 持独占租约，不增规划／模型请求。旧检查点准入前恢复原范围。见[上下文](context.zh-CN.md)、[工具](tools.zh-CN.md)、[ADR 0009](adr/0009-execution-invariants.zh-CN.md)、[ADR 0014](adr/0014-advisory-execution-policy.zh-CN.md)。

<a id="optional-model-selected-delegation"></a>
### 可选模型自主委派

`SubagentConfig` 默认深度 1、并发 8。具 `ChildHost` 的轮次经轮次级准入／取消适配器注册 `subagent`、`subagent_fork`，深度 0 禁用。运行时 `spawn_agent` / `wait_agent` / `cancel_agent` 复用 `ChildHost::prepare` 及 `AgentSupervisor::run_child`，后者即现有检查点 Loop，无第二循环／规划请求。适配器有界准入，不向父事件传播子内容／推理，仅紧凑生命周期。低于绝对深度且继承工具允许时子重绑委派。后代及队列子共享池和深度，嵌套耗尽拒绝避免祖先死锁。普通子树最多准入 64，队列总批数不受 64 限制。主模型跨研究、写作、分析、规划、编码判断，无语义分类或角色专属拆分。Host 配置重绑直接父工作区，CLI 每轮重载全局及项目覆盖。见[工具](tools.zh-CN.md#subagents)、[ADR 0010](adr/0010-optional-subagents.zh-CN.md)、[ADR 0023](adr/0023-scoped-subagent-depth.zh-CN.md)、[ADR 0030](adr/0030-unified-model-selected-delegation.zh-CN.md)。

<a id="capability-scope-composition"></a>
## 能力作用域组合

Skill/MCP/命名 Agent/Mod 经 `scoped::ScopedRegistry<T>` 解析，CLI 提供来源适配及共用管理器。先按名／ID 合并全局项目，再项目覆盖及继承屏蔽，仅启用进入执行。Crew 消费 ACP 行及管理边界，调用才读 Agent 指令。可移植能力配置带项目 UUID，运行存储保留原归属／迁移。见[能力](capabilities.zh-CN.md)、[ADR 0011](adr/0011-scoped-capabilities.zh-CN.md)。

可选 `RuntimeExtension` 同步有效上下文、适配提示、包装批准工具、观察完成。CLI `mods` 用惰性会话 Node 实现，命令／自定义工具经此执行；查询不 import JS，无 Mod 不启 worker。Node Mod 需 off，拒绝本地 SSH。见 [mods.md](mods.zh-CN.md)、[ADR 0022](adr/0022-lazy-session-mods.zh-CN.md)。

<a id="workspace-runtime-boundary"></a>
## 工作区运行时边界

独立 `sandbox` 在 ToolRegistry 及 MCP stdio 下，Agent／子／Skill 在本地副作用前到 SandboxManager。桌面／受管浏览器走声明 `AuthorizedHost`，精确应用／网站授权独立于文件／终端及普通审批，二者都不改沙盒。主机服务无显式绑定不进子注册表。见[主机权限](host-permissions.zh-CN.md)、[ADR 0024](adr/0024-independent-host-access.zh-CN.md)。绑定工具保留管理器复用 Linux broker，子有自己的工作区，stdio MCP 各自管理器／PID。会话以 `spawn_blocking` 准备，不前台等待；安全查询不准备，失败可重试。命令用私有 socket 及类型结果。Strict 禁网络，workspace 允许；权限独立。后端、生命周期及平台见[安全](security.zh-CN.md)、[ADR 0013](adr/0013-workspace-runtime-sandbox.zh-CN.md)。

当前 coding 策略、任务 Git 工作区、可选 Goal 及产物暂存导出见 [coding-harness.md](coding-harness.zh-CN.md)。

<a id="native-provider-dispatch"></a>
### 原生提供商分发

`model::provider_adapter` 构造 Chat Completions、Azure Responses 或 Anthropic/Gemini/Vertex/Bedrock/Radius。CLI 推理与目录刷新共用构造，原生模块负责协议和认证，AWS/Google 凭据惰性。可选助手 `provider_metadata` 经检查点和预算保留签名块，不增调用。见[提供商](providers.zh-CN.md)、[ADR 0020](adr/0020-native-model-providers.zh-CN.md)。

<a id="optional-distributed-collaboration"></a>
## 可选分布式协作

CLI 仅有 worker 环境绑定时注册 `collaboration`。`distributed_client`、`distributed_tool`、`distributed_worker` 提供作用域 REST、工具适配和按租约 ACP。核心推理、Subagent、Task、Session、Memory、Skill、MCP 不变，`ax crew worker` 独立于设备桥接。见[分布式](distributed-collaboration.zh-CN.md)、[ADR 0021](adr/0021-durable-distributed-collaboration.zh-CN.md)。

<a id="crew-workspace-discovery"></a>
### Crew 工作区发现

ACP 委托 acp_workspace.rs 做只读目录发现，规范路径、strict 边界。crew_device.rs 心跳发布项目根，Crew/SSH 选择主机目录，不读网关本地路径、不改 Session cwd。

SSH 使用 ssh/web 和常规运行时读器的独立工具集。本地 AX 驱动远端 POSIX shell，本地推理／历史。远端 cwd 为独立于进程工作区的元数据。SSH 声明远主机资源、权限及固定本地工具池豁免，同主机依赖／审批仍生效。并行 ACP 初始化用 immediate SQLite 事务及加锁原子项目注册。

<a id="optional-long-lived-goal-context"></a>
### 可选长期 Goal 上下文

`runtime-core::long_project` 管显式轮换、安全边界、有界交接。CLI `long_project` 投影 Markdown，经既有 ReplState/MemoryStore 提交普通新 Session。维护不需模型／后台服务，执行仍归 Goal/Task Queue/Harness，可复用 Memory 独立。Crew 现有 Workflow 隔离可携文档接管，见 [Long Project](long-project.zh-CN.md)、[ADR 0027](adr/0027-long-project-session-rotation.zh-CN.md)。

`distributed_session` 将 ACP 实时 Session 桥接到 Crew 既有 attempt/event/inbox；`distributed_settings` 在主动启用目标作用域下调用验证后的原生设置／能力。二者不改 Core 推理，不建 Session 存储或调度器。原生 Goal／文档修改报告成功前还调用 `distributed_goal` 检查点 CAS。
