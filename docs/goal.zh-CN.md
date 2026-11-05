<a id="ax-goal"></a>
# AX Goal

[English](goal.md) | **简体中文**

Goal 由原 StopGuard 就地演进，只有一个完成验证器和既有模型／工具 Agent Loop。它可选，普通任务默认不自动使用。Harness 负责可靠执行，Task Queue 负责任务身份和子任务，Goal 负责验收策略，Runtime 用现有预算和用量统计强制执行约束。

模型模式验收与任务执行共用提供商传输。Chat Completions 推理元数据保留在执行证据，在验收时为相同提供商／模型回放，见[提供商回放与旧 Session 限制](providers.zh-CN.md#provider-abstraction)。

<a id="use"></a>
## 使用

- `/goal <target>` 创建并执行自然语言目标，模板关闭时采用模型模式。可在目标中写验收要求，或在下述 JSON 的 `acceptance` 中提供字符串。
- `/goal` 或 `/goal status` 显示目标、状态、上次决策及消耗。
- `/goal pause` 保留未完成工作，`/goal resume` 显式恢复同 ID；需先回答挂起用户问题。
- `/goal cancel` 或 `/goal off` 取消可选目标。后续普通提示只在自动 Goal 开启时用当前作用域模板，绝不继承上一个标准。
- `/goal create {"target":"produce report","mode":"deterministic","deliverables":["report.json"],"max_rounds":20}` 设置每目标标准和限制。相对交付物路径按项目根解析，JSON 命令内路径亦如此。
- `/goal limits {"max_rounds":64,"max_tokens":200000}` 修改暂停或阻塞目标上限。消耗保持累计，提高上限不重置用量或证据，随后显式恢复。
- `/goal acknowledge <validator-id>` 记录对中断不安全验证器外部副作用的明确审阅，只清除重放屏障，不执行任何操作；需先检查保存调用及真实外部状态。

TUI 和 `ax run "/goal ..."` 均可使用。进程退出后经 `--session` 重开保存会话检查／管理。活跃 TUI pause/cancel 中断现有轮次句柄并记录中断结果，流式时 status 仍可用，其他文字继续作为补充指令。ACP 共用提示边界，接受现有 `_meta.axGoal` GoalTurn：带 `config` 和可选调用方 `goal_id` 的 `pursue`，以及 `resume`、`pause`、`cancel`、`answer`。原生 `/goal` 提示保留已有调用方 `Start` 身份，不创建竞争 Goal ID。可选状态／用量经 `session/update` 元数据提供。结构化 ACP `answer` 保留传入 Goal／问题标识和载荷，共享入口不针对其他挂起问题重解释附带文本，内核按保存问题验证。

<a id="configuration-and-migration"></a>
## 配置与迁移

全局 `$AX_HOME/config.json` 使用 `goal`，项目可在 `.ax/goal.json` 覆盖。模板配置显式创建及可选普通新任务。已有和新配置的 `enabled` 都默认 false。AXCrew 设置 → Goal 启用后，共享 CLI/TUI/ACP 将解析后的全局／项目模板应用于每个普通新任务，此路径 `mode: off` 使用模型验证，提示成为目标。修改下一新任务生效，不改变保存 Goal、显式恢复／暂停／取消、挂起答案或活跃补充指令。禁用保留规则及显式 `/goal`。项目模板整体替换全局策略，含开关，重置则继承。普通准入不迁移旧项目验证文件。控制器管理的分布式 worker 和本地子代理保留传入策略，不自动附加主机模板验收。

```json
{
  "enabled": false,
  "mode": "model",
  "acceptance": ["report agrees with actual measured results"],
  "deliverables": ["report.json"],
  "required_successful_calls": [],
  "validators": [],
  "validation_combination": "all",
  "max_rounds": 0,
  "max_seconds": 0,
  "max_tokens": 0,
  "max_cost_microusd": 0,
  "input_microusd_per_million": null,
  "output_microusd_per_million": null,
  "max_stagnant_checks": 3,
  "stagnant_seconds": 300,
  "max_unobserved_checks": 12,
  "stagnation_action": "pause"
}
```

读配置时全局 `verification` 原子迁为 `goal`，保留未知用户字段，已有 `goal` 优先。TOML 兼容使用同一别名，写新 JSON 名。显式创建 Goal 时，若新文件不存在，将旧 `.ax/verification.json` 原子转为 `.ax/goal.json`，旧文件保留备份。Core 只接收 GoalConfig，不含旧配置分支。旧持久 pending/allowed 标记仍支持子恢复。

`ax goal-settings --scope global|project` 不打开提供商或 Session 即读取同一模板。`--json-stdin` 从 stdin 接受最大 1 MiB 经验证 GoalConfig JSON，整体替换该作用域；`--reset` 恢复全局默认或移除项目覆盖。重置归档旧项目模板，防止静默再迁。保留其他配置。项目文件整体替换全局，缺字段用 GoalConfig 默认。可移植相对引用在创建时按选定项目解析。设置仅影响新任务／Goal，保存 Goal 保留冻结配置。

<a id="configured-validators"></a>
## 配置验证器

现有 GoalEvaluator 外包配置验收条件，不另建循环或工具执行器。确定性 Goal 可用自定义条件，无需声明文件或成功调用 ID；模型模式在自定义条件通过后仍独立评估。

```json
{
  "mode": "deterministic",
  "validation_combination": "all",
  "validators": [
    {"id":"build","version":"v2","kind":"command",
     "command":"python scripts/verify.py","reference":"scripts/verify.py",
     "timeout_seconds":30,"replay_safe":false},
    {"id":"ci","version":"v1","kind":"http",
     "url":"https://ci.example.org/ax/acceptance","timeout_seconds":30},
    {"id":"enterprise","version":"v3","kind":"host","tool":"mcp",
     "input":{"action":"call","server":"enterprise","tool":"acceptance",
              "arguments":{}},"timeout_seconds":30,"replay_safe":false}
  ]
}
```

Command 调用注册 `shell`，JSON 请求经 stdin 传入，不插值进 shell 文本。`reference` 指已有验证器文件，执行前和恢复后检查。HTTP 经现有 `web.fetch` GET，JSON 请求放 `ax_goal` 查询参数，响应须完整且为 `application/json`，继承网络策略、重定向检查和响应限制。Host 用 `ax_goal` 参数调用已有注册工具／MCP；惰性 `mcp` 网关将上下文放 `arguments.ax_goal`，`action` 必须 `call`。支持直接 JSON 回执、标准 MCP `structuredContent` 或单个文本块，MCP `isError` 不能准许完成。

三者均用 Scheduler 权限准入、资源租约和沙盒化注册。配置不能把未绑定工具移出隔离、注入调度控制字段或授予权限。超时 1–86400 秒，默认 30，受当前 Runtime 工具超时约束，涵盖审批、资源等待和执行。原始结果、调用 ID、类型化回执经现有 Session/Task Queue 证据路径保存检查点。

请求含 `protocol: "ax.goal.v1"`、`goal_id`、`target`、`acceptance`、`validator_id`、`version`。成功命令 stdout 必须恰一份 JSON 回执，日志可发 stderr；HTTP/host 返回同格式：

```json
{
  "protocol":"ax.goal.v1", "goal_id":"<request goal_id>",
  "validator_id":"build", "version":"v2",
  "decision":{"status":"allow"},
  "evidence":{"exit_code":0,"artifact":"report.json"}
}
```

`allow` 需非空证据；`continue` 表示验收未完成，`insufficient_evidence` 表示目前无法建立证明，`blocked` 需外部动作／权限，`failed` 为明确不可恢复结果。后四者需非空 `reason`。传输错误、格式错误或截断均为证据不足；标识／版本不匹配和类型化权限拒绝会阻塞，绝不静默通过。`all` 需全部条件，`any` 需一个有效条件，两者都不能隐藏阻塞或不可恢复失败。组合后仍需满足配置文件／调用要求。

Goal 持久保存完整规则、声明版本／引用、工具 schema、命令系统／架构绑定、回执及在途验证器。恢复重新检查可用性／绑定；真实服务可用性由有界调用建立，不从旧成功假定。主机 GoalEvaluator 实现需在新主机重新安装相同 `policy_reference()`。Command/host 默认不安全重放，中断后保留屏障直至明确 acknowledge；只读 HTTP GET 可安全重放。策略作者可将幂等 command/host 显式标记 `replay_safe`。

<a id="completion-and-limits"></a>
## 完成与限制

确定性模式检查声明交付物存在性及属于当前目标的结构化成功**调用 ID**，不把助手声称或 shell 文字当成功。存在不证明新鲜或语义正确。空检查集，或无自定义验证器却仅含自然语言验收，返回证据不足而非成功。

模型模式先检查确定性要求，再独立发流式提供商请求，唯一 schema 是 `goal_decision`：allow、continue、blocked、failed、insufficient_evidence，附具体原因。接收当前目标、验收、执行证据和子回执，不能执行工具，文字不成为第二答案。Continue／证据不足将原因反馈现有循环。Blocked 需外部动作和显式恢复，failed/cancelled 为终态。类型化权限拒绝阻塞 Goal，不增加授权。

轮次／时间默认**无限**，即零。Goal 可配置而不改变普通任务。Runtime 按累计剩余 Goal 轮次约束既有 `ExecutionBudget.max_steps`，剩余活跃时间走既有超时路径。一条执行或验证请求链算一轮，有界重试在步骤内。现有工具、子任务和执行预算仍生效；子执行有独立既有预算和调度，Goal 不增分布式全局 worker 账本。已完成子指标从当前队列回执仅吸收一次供显示。

报告用量来自现有 `ModelResponse.usage` 和子指标。仅每条计入请求都有完整 Token 报告时才强制 Token 限制；缺报告、重试、中断、语义压缩或委派用量使聚合成为保守／估算，关闭硬 Token 上限。费用限制需完整提供商 `cost_microusd` 账单。显式每百万 Token 微美元费率只生成显示估算，**绝不**作为硬准入。缺计费／费率显示不可用，无费率仍可配置费用上限。不推断发票价、缓存折扣或任意工具内部模型费用。报告上限耗尽后停止后续工作；在途请求可能超限，因为响应前未知最终用量。

停滞结合 Harness `ExecutionState.progress.advances`、实际队列／子状态转移、交付物元数据和新验证器工作观测。回执可含 `work`，其中 `status` 为 `running`、`waiting_external`、`no_progress`、`blocked`、`unknown`，含具体 `reference`、可选实测 `progress` 和 `valid_for_seconds`。观测到期，新鲜期最多为 30 秒与配置空闲间隔的较大者。未知证据绝不算失败。等待内联工具和子收集，审批、运行子任务和重试不计停滞。观测到后台／外部工作会保留活动及证据并挂起，不消耗推理轮询；真实变化后由用户／主机恢复，不增后台调度器。

默认需三次不变观测**且**300 秒活跃空闲，12 次不变为有限回退。`max_unobserved_checks` 不低于 `max_stagnant_checks`；空闲间隔可零以复用旧即时阈值，`max_stagnant_checks: 0` 显式关闭检测。默认 `pause`，`request_human` 为阻塞等待证据审阅。重复助手声称、工具失败或未变读取不算进展。资源耗尽／停滞只暂停，不完成。提供商走现有有限 RetryPolicy：瞬态执行错误耗尽后暂停，权限／配置故障阻塞，不可恢复错误失败。验证器失败绝不静默允许完成。用户可查证据、改上限并显式恢复。

<a id="persistence-and-recovery"></a>
## 持久化与恢复

既有 `[ax-task-queue]` 检查点的单一可选 `goal` 含目标、标准、状态、消耗、验证、证明 ID、Harness 进度基线和待估算。外层队列提供 Harness/Task/Session/AXCrew 共用 `goal_id`/`parent_goal_id`，规划不能替换该对象。不增数据库、分发器、记忆或分布式协议。

推进前，推理估算及工具证据经现有 JSONL 优先 sink 保存。中断恢复为暂停，不自动执行。未知在途用量保守显示估算，不设硬 Token／费用上限；中断到恢复的墙钟时间保守计入，完整暂停不计。单机 CLI 执行及 Goal 编辑用进程死亡即释放的 OS Session 租约。打开他人拥有的 Goal 会话不能修复／写历史，执行也拒绝过期 Goal／配置／用量快照。加载不执行，嵌入方自行负责会话所有权并选择显式恢复。

完成子回执复用。待工具调用变为未知副作用占位，并要求重试前检查外部状态。AX 无法独立证明全部外部副作用，不盲目重放。持久 allow 标记可免模型请求恢复已验证答案；已接受指令和待工作仍须完成消费。流式输出先于验证，只有成功准入才发 Completion 和 TurnFinished。

嵌入方用 `GoalTurn::Pursue`、仅配置下一目标的 `configure_goal`、`goal()` 及 `GoalEvaluator`/`with_goal_evaluator`。恢复后须显式重装自定义 evaluator，它是可执行主机策略，不是可序列化配置。AXCrew 仍负责分布式调度、所有权、故障检测和接管。

<a id="axcrew-durability-boundary"></a>
## AXCrew 持久化边界

既有分布式 worker 将显式 `/goal` 绑定 Durable Task ID，通过现有 Workflow CAS 及来源 lease/incarnation/generation 检查，在 `Workflow.state.ax_goal_runtime[task_id]` 保存队列、累计用量／预留和有界既有 Message 证据。模型请求、副作用及报告完成前均保存检查点。接管时缺权威状态则停止，不重置消耗。普通分布式任务不写 Goal 检查点；协作检查点保留 Goal 保留键。Crew 既有 256 KiB Workflow 限制生效，保存失败阻止推进。

中断的运行 Goal 可走现有 Crew 接管策略。主动 paused/blocked 在无人值守重试前需显式 worker `resume_goals: true`，默认 false；不清不安全副作用、不绕验证器或重置上限。暂停／阻塞仍在 Workflow；失败 worker 尝试走已有失败报告，不新增 Goal 调度器。

不迁移完整本地 Session 库、子会话或工作目录副作用。未完成 Goal 如有普通工具调用或本地子任务，会阻止自动接管，需主机协调产物／副作用。保存工作区外路径策略被拒绝；相对工作区引用在新主机重定位并验证。验证器可用和预算不变是前提，不假定机器一致。最终提交仍受 Crew 当前任务所有者和 generation 隔离约束。

设计参考：[Claude Code Goal](https://code.claude.com/docs/en/goal) 及 [Hooks](https://code.claude.com/docs/en/hooks)。AX 保留自身 evaluator、Runtime 权限／预算和 AXCrew 所有权边界。

<a id="long-project-and-multi-session-execution"></a>
## Long Project 与多 Session 执行

显式 Goal 可启用 `long_project` 及其默认自动 Session 轮换。标准、累计用量、状态和权限上限跨轮换保留，工作 Markdown 仍是非权威证据投影。原生 `/goal project` 管理文档／配置，`/goal rotate` 请求安全交接。普通任务除非显式配置轮换，否则不变。默认、作用域覆盖、导入／导出、冲突和恢复见 [Long Project 与 Session 轮换](long-project.zh-CN.md)。
