<a id="context-compression--resume"></a>
# 上下文、压缩与恢复

[English](context.md) | **简体中文**

本文介绍 AX 如何分配模型上下文窗口、在过大时压缩，以及跨运行恢复会话。底层存储见 [storage.md](storage.zh-CN.md)。

<a id="overview"></a>
## 概览

“上下文”是某轮实际输入模型的消息。AX 不删除原始历史，只管理模型*可见*范围，并在压力增加时压缩可见部分。

上下文是**默认可用的支持材料，绝不是任务**。每轮注入有界环境（cwd、工作区根、沙盒状态）及存在的适用工作区指令（`AGENTS.md`、`.ax/rules`），相关时加入记忆、会话历史和旧产物。这不意味着用户要求检查、修复、继续或修改任何内容；当前请求定义工作，仅请求需要时上下文才触发动作。见 [agent-runtime.md](agent-runtime.zh-CN.md)。

<a id="contextbudget"></a>
## ContextBudget

AX 使用一个弹性输入池：

```text
provider context window
  - hard output reserve (model max_output_tokens; fallback 8000)
  - actual tool schema estimate
  = usable shared context pool
```

不按百分比分割 Skill/Memory/Summary。旧预算访问器返回同池上限，不是可相加分配。`ContextDemand { demand, minimum, maximum }` 表达实际需求，`allocate()` 满足全部硬最小值，拒绝不可能的最小需求，再按调用方优先级分剩余空间。运行时、能力、harness、环境、项目指令及运行进度是硬最小；选中技能和相关记忆先于旧可选历史竞争。请求选择保留完整对话轮次，含最新用户轮次。历史摘要及其他可选系统上下文只能使用选中近期对话后的剩余空间。

`.ax/config.json` 中 `context_pool` 集中定义操作上限及下一请求预留：

```json
{
"context_pool": { "next_request_reserve": 1024,
                  "tool_result_maximum": 4096, "recent_raw_maximum": 4096,
                  "memory_maximum": 8192, "skill_metadata_maximum": 4096 }
}
```

<a id="minimum-window-capacity"></a>
### 最小窗口容量

共享 `ContextBudget::validate_capacity` 根据各阶段必需输入检查容量：

```text
minimum context window
  = output reserve
  + actual tool schema estimate
  + next-request reserve
  + minimum required input
```

模型步骤、压缩或 Session 轮换前，预检只计各保留策略都必须保留的运行时、能力、harness、环境和项目指令。不把旧进度或用户补充指令视为不可约减：进度会刷新，`retain_user_guidance` 为 false 时交接可省略补充指令。可缩减工具输出也不计入固定最小值。

请求组装在选上下文前验证必需运行时消息、当前进度及最新用户消息。交接验证完整后继上下文，包括策略保留材料。因此预检通过不保证具体请求或交接能放下。最小值取决于模型、启用工具、指令和输入，`--context-window` 没有通用支持下限。

阶段必需成本超过窗口时，在发送提供商或激活后继 Session 前返回 `context window too small`。不可能的固定容量在压缩／交接构造之前预检失败。诊断列出窗口、输出预留、工具 schema、下一请求预留、最小输入和最小总需求。应增大 `--context-window` 或选适合的模型／工具配置；不静默扩大窗口或丢必需指令，压缩／轮换不能回收固定最小成本。容量错误不证明尝试或完成了轮换。

请求前，压缩投影该请求实际新增内容：压力检查后追加、受 `tool_result_maximum` 限制的队列摘要上下文，加下一请求预留。按 `usable()` 检查预计输入，不按当前占用百分比触发；硬压力边界即有效池。手动压缩共用管道。自动压缩不由百分比阈值或固定比例 helper 决定；清理保留量来自 `tool_result_maximum`，非私有字符数。极小窗口使用饱和运算。

<a id="context-selection"></a>
## 上下文选择

`context::select_context` 按 **Token 预算**而非固定消息数选择恢复历史，保证不截断未完成工具调用轮次，用于会话恢复及每次请求组装。

<a id="tool-result-projection"></a>
### 工具结果投影

每请求前，`ToolResult::model_view` 用 `ContextBudget::tool_result_chars()` 限制可见正文。工具经 `ToolOutput::Verbatim` 声明逐字源码，调度器在无损信封保存 `verbatim`、`total_lines`、`start_line`、`truncated`、`line_truncated`。旧信封缺新增字段时按普通文本反序列化，不靠工具名判断源码。

Verbatim 保留精确文本，含 JSON 空白、`warning:` 行、CRLF 和末尾换行；文件显示前缀 `N: ` 携带原坐标，源码不转成 `diagnostics`。过大正文在共享配额内按完整行截断，提供 `total_lines`、`shown_through_line`，截断时提供 `next_start_line`，可精确续读。单行超过配额则在 UTF-8 边界保留行首，设 `line_truncated`，`next_start_line` 指后续行，无后续则 null。请求范围完整时即使文件有后续行也返回 null；`next_start_line` 只表示预算／字节截断。`search` 定位过大行的深处内容。非逐字结果仅失败时推断诊断，显式 JSON 诊断数组成功时也保留，并保留 JSON 格式化及警告清理。

<a id="compression"></a>
## 压缩

压缩只改模型输入。原始消息保留 SQLite/JSONL，只按会话 upsert 最新累计摘要并推进水位。

自动与手动共用**分层管道**：

1. 清理工具输出；
2. 去重；
3. 仍有需要时语义压缩。

清理识别已存 Verbatim 声明，保留完整开头行、原坐标及截断状态，不收集看似诊断源码或对正文做普通文本清理。压缩及保留轮次缩减使用 `compact_verbatim_lines`，不用部分首行读取投影：首个完整行放不下时输出为空、`truncated` 为 true、`line_truncated` 为 false，续读仍指首行。非逐字 JSON 视图保持有效 JSON，仅压缩 `output`，保留开头／诊断样式行及不变 `diagnostics`；只有不可解析文本走文本回退。同轮发现复用输出简短 `reused_from`，不重复正文，保留 status/truncated。类型化依赖解析原回执，不改原历史。请求准备和 Session 轮换保留类型化信封，不从原文本重建，后续请求维持源码语义。语义压缩仍可用摘要替代旧对话，但权威原结果始终可用。见 [ADR 0031](adr/0031-tool-declared-verbatim-output.zh-CN.md)、[ADR 0032](adr/0032-verified-patches-and-bounded-receipts.zh-CN.md)。

保留输出缓存跨恢复保留 Verbatim 信封及坐标，兼容旧纯字符串。实时／恢复／旧条目插入共用由 `MAX_FILE_BYTES` 派生的 FIFO 上限，包含 ID 和序列化元数据。淘汰最旧回执，单份超限不保留；缺失／已淘汰 ID 明确要求重读或重跑。缓存仅便利设施，不是权威历史。大文件读取最多保留同一字节上限，另以规范路径、mtime、长度为键在进程内有界 LRU 缓存精确行数，命中后无需扫描所需范围以后，首次未命中仍扫到 EOF。恢复从已存消息重建缓存，可能已是压缩投影，因此截断信封不替换同调用完整回执。`tool_output` 对截断回执已不保留范围拒绝，不答空范围。文件回执范围采用原文件坐标，保留前缀及行尾；普通 shell 回执读取保持原格式行为。轮换每轮将结果减半，工作量随正文大小呈对数，同时只保留完整行。

**自动**压力检查在每模型请求前，以预计下一请求大小对比共享 `ContextBudget` 池。

**手动** `/compact` 立即压缩当前有效上下文：

1. 报当前估算大小，从运行时移除注入检索记忆。
2. 有运行时则调用 `AgentKernel::compact_now`，否则不压缩。
3. 内核运行同一分层管道。
4. 成功后 CLI 保存有效上下文快照和摘要，保留原消息。
5. UI 报前后估算 Token，或提示无内容可压缩。

<a id="sessions--resume"></a>
## 会话与恢复

- **`/resume`**：跨 `~/.ax/session-projects.json` 注册项目搜索近期会话，最多 100 条。Ctrl+P 按项目筛选，Ctrl+R 重命名，Ctrl+D 删除，Ctrl+N 新建。创建会话或打开 `/resume` 时注册项目。选择后切换工作目录和项目存储，有快照则恢复，再追加压缩水位之后消息。旧库回退摘要加未压缩消息。`select_context` 按当前模型预算限制上下文，UI 独立恢复完整已存历史。所选会话变当前，运行时按需重建，会话权限重置。恢复时 TUI 显示加载状态。
- **`/new`**：新会话清空活跃引用、加载消息、技能及运行时，重置会话权限。SQLite 行在下一提示惰性创建，标题为提示首个短分句。旧会话保留，可 `/resume` 重开。

输入框 `@` 按需搜索项目文件名，选中引用在项目根内解析，其 UTF-8 文本使用与技能相同上下文预留。跳过超大、非文本或项目外文件。引用正文作为会话 agent-state 保存以支持恢复，原用户消息仍保留 `@` token。

新流输出到达输入框上方时，TUI 保留阅读位置，底部提示未读输出，End 回到最新。崩溃前保存的中断工具调用以明确标记恢复，绝不自动重放，见[记忆](memory.zh-CN.md)。

<a id="related-slash-commands"></a>
## 相关斜杠命令

| 命令 | 用途 |
|---|---|
| `/compact` | 立即压缩当前有效上下文 |
| `/resume` | 打开旧会话 |
| `/new` | 新会话 |
| `/status` | 只读显示上下文使用、执行限制、预算和延迟快照 |
| `/exit` | 退出 TUI |

<a id="reference"></a>
## 参考

| 职责 | 代码 |
|---|---|
| 预算派生 | `crates/core/src/budget.rs` |
| Token 感知选择 | `crates/core/src/context.rs`（`select_context`、`request_context`） |
| 压缩管道 | `crates/core/src/compression/`（`pipeline.rs`、`summary.rs`） |
| Agent Loop、模型和工具步骤 | `crates/core/src/loop_runtime/`（`mod.rs`、`model_step.rs`、`tool_step.rs`） |
| 会话快照 | `crates/memory/src/context.rs`（`save_effective_context`） |
| 分发与选择器 | `crates/cli/src/tui/commands.rs`（`execute_slash`、`open_session_picker`） |
| 会话恢复 | `crates/cli/src/repl/state.rs`（`ReplState::open_session`、`reset_new_session`） |
| 恢复辅助 | `crates/cli/src/session_restore.rs` |

<a id="long-task-progress"></a>
## 长任务进度

多任务轮次保留持久内部 Task Queue。每执行模型请求前替换紧凑 `[ax-progress]`，含 `overall_goal`、`current_task`、`completed_count`、`failed_count` 和 `remaining_tasks`，后者是待任务编号。已完成正文及完整队列检查点不注入进度；当前任务保留所需细节，原请求提供待任务定义。进度和队列工具 schema 计入现有 `ContextBudget`，选择可选历史前预留进度。

全部任务完成、失败或跳过后，最终摘要请求只接收一次终态及简要结果。显式完成控制推进执行，任务未完成时纯文本答案仍保持 Goal 活跃并继续。终态响应缓存于持久状态，恢复 blocked/completed/cancelled 不重复输出。完整快照及归档 Goal 从模型上下文移除，worker 也移除控制器队列。压缩共用既有管道。

显式委派队列子任务默认仅获任务输入和少量运行时元数据，即会话、cwd、记忆作用域、平台及 shell。显式子代理 `ChildPolicy` 可选有界父上下文、技能与记忆继承，排除控制器队列和兄弟历史，见[工具](tools.zh-CN.md)。恢复子任务只重载自身原始历史和原结果读取器。禁用子队列工具，防止误编排控制器队列。父进度紧凑，摘要含子工作区／会话／状态引用和简要结果，不含完整子工具历史。

`--child-timeout-secs` 是每隔离子执行模型／工具的总截止，不因持续活动延长；默认无限，不隐式结束控制器。`--turn-timeout-secs` 是*空闲*截止，委派轮次覆盖整批子任务，各子独立预算；只有整个窗口没有模型轮次、工具轮次、子分发或回执才取消控制器。ACP 子分发也用这些选项。

各子回执为结构化 `ChildResult`。控制器模型仅获紧凑投影：状态、根因、相关文件、验证和建议下一步；发现、诊断、文件、diff 统计及指标持久保存，通过 `child_result` 按需读。持久索引每次分发写一次，不按子数反复写，n 个子仅需一份快照，不是 n 份增长快照。

<a id="project-instructions"></a>
## 项目指令

每轮解析一次项目指令，以独立可替换 `[ax-project-instructions]` 系统消息注入。由浅至深：全局指令文件、仓库根 `AGENTS.md`、根到 cwd 每级 `AGENTS.md`，同级 `AGENTS.override.md` 替代 `AGENTS.md`，最后为 `path`/glob 匹配任务文件的 `.ax/rules/*.md`，即 `@` 引用及仓库中实际存在的路径样式 token。

各段带 `source_path`、作用域、优先级和来源；最具体层级优先，按优先级再来源路径构成全序，相同树和提示生成相同指令集。不查记忆、嵌入或语义索引。Token 配额来自 `ContextBudget::instructions_budget_tokens()`（`ContextPoolPolicy::
instructions_maximum`，默认 4096）。始终启用指令链不丢弃；预算不足时省略最不具体的路径规则并报告。

指令每轮重解析，不持久保存，恢复会话时不会过时，也不写入原历史。它与记忆、技能和系统上下文是四个独立概念，不能相互替代。

<a id="asking-the-user-mid-run"></a>
## 执行中询问用户

`request_user_input` 挂起而非结束执行。内核为 Goal、队列、Session、问题保存检查点，队列停为 `waiting_for_user`，有意留下未答工具调用。前端显示问题，答案写回同一调用 id 的工具结果，`GoalTurn::Answer` 从该位置继续，不开新用户轮次。重连仍是等待，挂起不报告失败。授权不经此路径，危险操作由权限系统决定。

<a id="execution-state-and-truthful-tool-history"></a>
## 执行状态与真实工具历史

每工具完成后，内核经既有 AgentState/JSONL 保存 `[ax-execution-state]`，最多十六条完成事件，含调用 ID、工具、步骤、实际解析参数、结果预览及进度，另有累计工具调用数。完整结果留权威历史。CLI 独立于有效快照和有界历史页恢复最新检查点。压缩仅改对话，不能用模型猜测替代执行状态。

每执行请求获可替换 `[ax-execution]` 投影，含 Goal、步骤、观测状态、预期输出、作用域、恢复和真实事件。进度／恢复只是建议：空结果、无匹配和失败均是观测，不移除工具访问。普通请求显示最近两条，新用户轮次或停滞最多八条，按 `ContextBudget` 适配；也无需语言关键词匹配即可回答历史／进度问题。缺旧事件表示未知，不是零调用。旧／压缩会话无检查点时总数未知，仅回填保留的真实调用／结果对。子完成使用与 UI 相同会话前缀 ID，控制器游标避免重连重复计数；子状态与控制器独立。

<a id="optional-session-rotation"></a>
## 可选 Session 轮换

启用 Long Project 或显式独立轮换策略，可在安全模型边界将高压有效上下文换到普通新 Session。压力和压缩收益从 ContextBudget 派生，不按固定消息数。旧原始流、Goal／队列标识、预算、权限及完成回执不变。最新轮 `tool_output` 以可选 agent-state 保存，旧证据按需从既有 Session 读取。不可能窗口在交接前由共享容量检查拒绝；交接必需上下文仍放不下则暂停，不静默移除。见 [Long Project](long-project.zh-CN.md)。默认普通循环无文档、轮换 host 或额外模型请求。
