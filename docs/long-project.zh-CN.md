<a id="long-project-and-automatic-session-rotation"></a>
# Long Project 与自动 Session 轮换

[English](long-project.md) | **简体中文**

Long Project 是可选 Goal 工作上下文投影，默认关闭。显式启用后，自动轮换默认也开启。两者均不新建代理、调度器、记忆服务或执行数据库。Goal、Task Queue、Harness/Checkpoint、既有 Session JSONL 和 SQLite 仍是执行／恢复权威。

<a id="enable-and-manage"></a>
## 启用与管理

在 TUI、`ax run` 或 ACP 用现有原生命令创建显式 Goal：

```text
/goal create {"target":"maintain the project","acceptance":["changes are verified against measured results"],"long_project":{"enabled":true}}
/goal status
/goal pause
/goal project {"enabled":true,"rotation":{"automatic":false}}
/goal resume
/goal rotate
/goal project show
/goal project export project-state-copy.md
/goal project import reviewed-project-state.md
/goal project history receipt-reference
```

保存 Goal 的配置只能在 paused、blocked 或 created 状态修改，不执行工作，之后需显式 resume。手动轮换恢复未完成 Goal，并在下个安全模型边界请求交接，自动轮换关闭时也可使用。待回答问题需先回答，终态 Goal 不能通过轮换重启。

`project show`、导入／导出和归档搜索是无需提供商的原生命令。文档是普通 Markdown，也可编辑器修改。导出需新目标，导入归档旧文档，不替换标准、预算、权限或 Task 状态。用 `{"enabled":false}` 禁用，保留文档／历史，重启用可继续。AX 不通过分类普通用户提示自动启用。

<a id="configuration-scopes"></a>
## 配置作用域

现有全局 `config.json` 的 **goal** 模板和项目 `.ax/goal.json` 接受 `long_project`，嵌套字段按全局 → 项目 → 每 Goal JSON 合并，显式 `null` 移除继承配置。其他 Goal 字段仍整体项目模板替换。保存 Goal 冻结解析后配置。`ax goal-settings --scope global|project` 共用验证及持久化路径，编辑模板不执行任务。

```json
{
  "long_project": {
    "enabled": false,
    "directory": null,
    "template": null,
    "sections": ["目标与约束", "当前状态", "已完成工作", "待办任务", "关键决策", "问题与风险", "运行环境", "历史索引"],
    "save_content": ["completed_work", "todo_tasks", "tool_references", "decisions", "issues", "environment", "history", "user_guidance"],
    "auto_update": true,
    "update_on": ["state", "stage", "decision", "handoff"],
    "max_document_bytes": 65536,
    "evidence_entries": 24,
    "stale_after_seconds": 86400,
    "rotation": {
      "automatic": true,
      "strategy": "after_compaction",
      "context_percent": 85,
      "minimum_compaction_gain_percent": 20,
      "retain_recent_tools": true,
      "retain_user_guidance": true,
      "retain_compaction_summary": true
    }
  }
}
```

`template` 为字面 Markdown，`{{goal_id}}`、`{{target}}`、`{{acceptance}}` 安全替换，其他自定义字段／文本属于用户。无模板时用八个配置标题构成内置模板。用户可重命名、增删非必需章节，通过 `long_project` 工具写对应证据笔记。必需运行时检查点引用独立于模板，删标题不能删执行身份或让文字成为权威。

`save_content` 按上述名称选可选投影字段，可空列表生成最小文档。Goal 标识、目标、验收、当前状态和检查点来源始终必需。自定义字段是字面模板或证据笔记。移除投影字段不删旧归档／用户正文，不改权威 Task 状态。

`directory` 为显式可选基目录，可相对项目，或共享文件系统绝对路径；默认 `.ax/long-projects`。每 Goal 使用 `<base>/<sha256(goal_id)>/PROJECT_STATE.md`，避免标识路径遍历和 Goal 间冲突。字节上限为 1 KiB–1 MiB 存储限制，不是上下文策略；模型摘录始终按 ContextBudget。`evidence_entries` 限制当前任务／调用引用，完整记录仍在现有检查点及历史 Session。整文更新超限时失败，保留旧文档。

普通任务轮换需另行启用：全局 `session_rotation` 使用同一轮换对象，默认 `null`。显式 Goal 可用 `GoalConfig.session_rotation` 而不启 Long Project。已启 Long Project 优先用自身轮换配置，否则 Goal 覆盖优先全局；这不启 Long Project 或创建文档。

<a id="evidence-and-incremental-document-maintenance"></a>
## 证据与增量文档维护

Host 投影真实队列状态、依赖索引、Goal 状态、验收、结构化成功调用 ID、活动、决策及来源 Session／检查点。任务按既有队列顺序，不增加并行优先级／任务系统。完成状态和成功调用仅表示保存回执证明的内容，不把助手声称当语义验收。交付物、提交、环境事实和产物可用证据笔记引用，不复制完整工具输出。

`long_project` 仅为已启用 Goal 注册，经现有 Scheduler、资源租约、能力权限和运行时自有文件系统边界，不继承给子执行。动作：

- `record`：章节、简短文本及 `sources`，来源为当前 Goal 成功调用 ID 或完成 `task:N`。未知／失败来源和敏感内容拒绝。笔记明确是需核对来源的代理注释，绝非完成证明。可给 `expected_revision`，否则用 host 上次观测修订为基线；冲突需读并合并。
- `read`：文档、修订和新鲜度元数据。
- 带 `query` 的 `history`：按需检索归档上下文。
- 带 `session`、`call_id`、可选 `before_message_id` 的 `history`：读取现有原始 Session 工具回执，不重执行。Session 需同 Goal 标识，调用在当前 Goal 检查点须成功。既有 MemoryStore 分页限制单次读取，需要下页时返回续读游标。

自动更新比较真实状态签名，只在配置状态／阶段／决策边界或交接更新管理块，单纯助手文字不触发。`auto_update:false` 关闭普通刷新，首次初始化及必需交接引用仍保留。旧版归档至 `archive/<revision>.md`。每章节／参与者当前一条笔记，替换该参与者上一条完整笔记，旧版可检索。模板生成、投影刷新、归档及上下文转移都不调用模型。

管理块带内容摘要。人工编辑块逐字节保留，新运行事实写独立当前块，自由文本绝不无条件重生成。各 writer 在文档租约内重读，替换前比较预期字节。运行／笔记块按参与者首个 Session 身份隔离，并贯穿其轮换链；不同 AX 合并自身块，不覆盖他人状态。大小、租约、修订或权限错误以明确可恢复错误阻止推进。

自动文字处理省略疑似凭据行、PEM 和不透明疑似凭据值，不自动复制原始输出。导入、自动归档和分布式文档快照也过滤。过滤保守，本功能不是凭据保险库，用户原始文件仍归用户。

带时间戳观测有可配到期检查。过期、无时间戳或人工上下文使用前需核实当前回执和实际环境，改时间戳不等于重新验证远端。按需通过普通工具验证。模型有界摘录中，决策注释放在可能很长自定义模板之前；省略内容明确提示如何检索。

<a id="rotation-and-recovery"></a>
## 轮换与恢复

循环在既有 Compaction 后、下个模型请求前评估策略。压力为有效上下文估算 Token 加真实预计请求增长，除以 `ContextBudget.usable()`，不含回复／schema 预留，不以消息数作唯一触发。

- `pressure`：达到配置压力时轮换。
- `after_compaction`，默认：高压下实测压缩收益不足，或没有适用压缩仍高压时轮换。
- `stage_boundary`：高压下，较上次交接又完成队列工作时轮换。同一完成阶段不反复触发；待任务仍待执行，不重新提交完成工作。

自动交接还须释放配置的最小上下文比例，防止持久指令占主导时反复空轮换。压缩／交接前共享 `ContextBudget` 预检要求可容纳输出预留、实际工具 schema、下请求预留及持久运行时／能力／harness／环境／项目指令。不计将刷新进度、策略可省略用户指令和可缩工具输出。低于固定最小时，在构造或压缩前以 `context window too small` 和开销明细失败。需增 `--context-window` 或选适合配置；AX 无法回收必需指令，也不静默增窗。见[最小容量](context.zh-CN.md#minimum-window-capacity)。

预检通过不保证完整交接可放下。运行时按保留策略构造后验证全部后继上下文，`retain_user_guidance` 为 false 可省用户指令；仍放不下则安全暂停并保留原 Session。请求组装另行验证运行消息、当前进度和最新用户消息。手动轮换不受自动收益要求约束。工具、审批、重试、用户问题、运行子任务及在途验证／预留均推迟交接，不中断运行工作。

保存／激活顺序：

1. 刷新旧原消息、当前 Goal／队列检查点和工作文档。
2. 构建有界上下文：运行／权限指令、Goal 定义、笔记、当前队列、必需 Memory；用户指令、明确未验证历史摘要和最新完成工具轮按保留策略。
3. 按 ContextBudget 检查完整上下文。大输出投影但保留调用／结果对，最新轮原结果用可选 agent-state 给 `tool_output`。
4. 初始化普通新 Session、有效上下文水位、队列、ExecutionState、子回执，保存前驱引用。
5. 在旧权威 JSONL 追加后继提交，之后才激活新 Session。保留相同内核、Goal ID、累计用量、验收、执行限制、权限上限、child host。
6. 清旧每轮原始缓冲和旧原结果缓存，保留最新轮。旧 Session 和归档上下文留磁盘。

轮次全程 Session 租约保护两侧。后继提交前崩溃以前驱为权威，初始化不完整后继不能打开／执行；提交后崩溃沿链到新 Session。旧引用解析到最新已提交后继以管理 Goal，历史流仍可查看检索。打开会话不自动执行，既有 Goal 恢复在显式 resume 前协调队列、子回执、在途用量及不安全验证器屏障。所有不确定性可见；轮换不批准完成、不重置消耗。

CLI/TUI/ACP 接收 `SessionRotated`，ACP `_meta.axSessionRotation` 含 Goal ID 和新旧 Session ID，原 ACP 对话可经已提交链继续用原句柄。终端输出不绕 TUI 事件渲染器。

<a id="distributed-use-and-limits"></a>
## 分布式使用与限制

共享文件系统须支持各 writer 的 OS 文件租约及原子重命名。既有权限／ACL 决定访问，错误拒绝继续，符号链接路径拒绝，模板不推导授权。人工编辑器应在 AX 下次更新前完成保存，其他编辑器不参与 AX 建议性租约协议。

现有 AXCrew Workflow 检查点在相同队列和有界消息旁可携带过滤的 `long_project_document` 及摘要。既有 revision CAS、owner incarnation/generation、gateway 权限继续隔离旧写入。Long Project 边界前运行时检查主机检查点接点。接管验证摘要并合并，不覆盖本地用户上下文。不增加 AXCrew schema、调度器、记忆或全局预算账本。

适配器不迁完整本地 Session 库或子工作区。既有接管仍要求跨机执行前主机协调普通外部副作用及本地子工作。分布式所有权仍归 AXCrew，共享 Markdown 不授权两控制器管理同任务。需要工作文档之外历史时，归档可经现有 Artifact 或共享目录转移。Long Project 管工作上下文，Memory 管可复用跨任务知识，Task/Checkpoint 管可靠执行恢复。

没有定时器、后台唤醒或 AX 关闭时执行保证。长期 Goal 经现有显式执行／恢复和主机调度继续。设计支持长期生命周期中的多个有界 Session，不要求永增上下文，也不声称已经观测过持续数月／数年真实运行。

AXCrew 协作工作区在已启 Goal 内联展示 Long Project。实时命令使用原生 project show/import/config/history 和 rotation，变更成功包含既有带隔离 Workflow 检查点，使可编辑投影和保存策略同步以供接管。报告后继 Session ID 延续相同尝试历史，编辑文字不重置用量，也不改变权威执行／权限。
