<a id="agent-loop"></a>
# Agent Loop

[English](agent-loop.md) | **简体中文**

本文介绍 AX 如何继续和结束模型轮次。更广泛的请求／能力契约见 [agent-runtime.md](agent-runtime.zh-CN.md)，任务队列、工作区和恢复见 [coding-harness.md](coding-harness.zh-CN.md)。

共享内核运行 `model → execute tools/tasks/children → append results →
model`。没有续接的最终响应结束轮次，不检查提示关键词或任务复杂度。普通成功回答和代码解释需要一次执行模型请求、零审查请求。当前轮次类型化可恢复工具错误会在提问前，或最新工具轮次仍含此错误的最终响应前，触发有界交接自审，见下文。

每轮均在 coding harness 下执行，由请求定义工作，运行时推进至终态。运行时提示和各能力指引在轮次开始注入，见 [agent-runtime.md](agent-runtime.zh-CN.md)。

执行模型跨任务领域决定是否委派有益子问题。普通子代理与队列子任务共用并发池、继承权限和绝对深度上限。类型化数据清单默认控制器执行，`task_queue action=configure` 可显式选子执行而不替换清单。恢复队列若当前设置禁止委派，则提供模型反馈，在控制器模式保留任务标识及回执。不引入分类器或额外规划请求。

<a id="runtime-state"></a>
## 运行时状态

`continuation::TurnState` 将待处理工具调用／结果、队列项、运行子代理、未消费子结果、待审批、用户输入／指令、重试活动、必需动作及提供商续接信号投影为：

```rust
enum TurnContinuation {
    Continue(ContinuationReason),
    Wait(WaitReason),
    Complete,
}
```

Continue 或 Wait 时 `needs_follow_up` 为 true。下一模型步骤前，调度器等待工具命令和审批 future，重试 future 仍在提供商步骤内。取消安全计数器跟踪审批和重试。现有有界 supervisor 等待队列子任务；可选子代理回执尚未返回时，最终完成前先收集。新结果强制模型步骤，成功消费后清除。`request_user_input` 以 `WaitingForUser` 挂起，在原调用 ID 恢复。Host 经 `kernel.turn_input().steer(...)` 排入实时输入；流式输出或阻塞守卫期间到达的输入在完成前消费。此 API 本身不启动轮次。

提供商结束原因 `length`、`max_tokens`、`incomplete`、`pause_turn`、`tool_calls`、`function_call` 要求续接。协议适配器验证自身完成边界：Chat Completions 流需终态 `finish_reason`，Responses 流需 completed 响应；仅传输 EOF 不算成功完成。若适配器已验证完成，内核仍接受不带 finish reason 的提供商中立响应。不匹配文字。显式取消、全局失败和执行预算保持独立停止／挂起语义。

CLI、REPL、TUI、`ax run`、ACP 及 AX Crew ACP 桥接共用此内核。前端渲染事件，不运行完成审查器或判定助手消息为终态。`ContentDelta` 随 Token 到达即转发，队列活跃时亦如此。`TurnFinished` 是 AX 轮次完成事件。

<a id="loop-hygiene"></a>
## 循环卫生

每轮检测连续相同工具调用，即相同工具和规范化参数。第 3、5、8 次各注入一次建议性 `[ax-loop-hygiene]` 系统提醒，要求检查前结果、换方法或认定任务已满足。调用仍执行，提醒只补充下一请求。不设很低全局上限，长而有效任务仍可多次调用。调用改变或每轮开始时重置链，见 `crates/core/src/loop_hygiene.rs`。

<a id="optional-goal"></a>
## 可选 Goal

普通成功任务直接结束，无额外验证或提供商请求。启用作用域 Goal 模板后，经 CLI/TUI/ACP 共享准入路径对普通新任务应用策略。显式 `/goal <target>` 为同一循环增加目标作用域确定性或独立模型验证；拒绝提供具体续接原因，blocked、failed、paused、cancelled 与完成区分。Goal 标准、限制及消耗和同一 Goal ID 一起位于现有队列检查点。自动模板默认关闭，已存 Goal 生命周期操作、挂起答案和 worker 策略不变。

命令、生命周期、资源准入、迁移和恢复见 [goal.md](goal.zh-CN.md)。`GoalEvaluator` 是由旧停止验证器演进的单一扩展边界，worker 不继承控制器标准。

<a id="observability-and-recovery"></a>
## 可观测性与恢复

各执行模型步骤发出 Continuation，可选 Goal 发出 GoalUpdated 和含结构化决定的 GoalEvaluated。完成报告 `model_steps`、`goal_model_requests`、工具和 evaluator，随后 TurnFinished。默认完成为 `direct`，验证完成为 `goal`。AX_EVENT_LOG 经共享 CLI/TUI/ACP 提示边界记录事件。验证器输出不成为第二份最终答案。

待交接审查、Goal 和提供商续接标记防止恢复过早子答案。只有持久 Goal allow 标记允许恢复验证过的最终答案。旧 completion_check 和 pending/allowed 记录仅为兼容数据，原始历史完整保留。

<a id="verification-and-references"></a>
## 验证与参考

续接验证覆盖请求次数、流式输出、待审批、重试及输入、队列／子完成、Goal 拒绝／恢复和重复调用提醒。普通成功路径保持请求数，可恢复错误经执行模型获得有界反馈。私有 fixture 路径及机器特定结果放本地日志。

设计参考：[OpenAI Codex Agent Loop](https://openai.com/index/unrolling-the-codex-agent-loop/) 和 [Claude Code Stop hook](https://code.claude.com/docs/en/hooks#stop)。AX 采用工具／结果驱动续接、有界错误驱动交接反馈和可选目标驱动 Goal，不把固定语义验证器复制进核心循环。

<a id="error-driven-handoff-review"></a>
## 错误驱动交接审查

`harness::handoff_review` 在问题边界读取当前轮次类型化失败回执，在最终边界读取最新工具调用轮次，返回抽样调用 ID 及恢复／报告指引。后续操作成功或队列结果已记录时，不因旧失败再做最终检查。首个有效问题返回 `review_required`，不发 UserQuestion，不把队列改为 WaitingForUser；后续必要提问可正常挂起。首个普通最终响应以 `[ax-handoff-pending]` 标为候选保存检查点，再给审查反馈，进入另一次执行模型步骤。只有接受的最终响应发 TurnFinished。

各边界每主机轮次只有一次额度，反馈不会形成无工具无界审查循环。模型预算已耗尽时不触发。不重复最终 Goal 评估；类型化权限拒绝／全局阻塞、旧轮次失败及文字不触发。既有补充指令、取消和审批规则仍生效。此机制鼓励恢复及基于证据报告，不能证明语义正确或要求外部凭据必然可用。见 [coding-harness.md](coding-harness.zh-CN.md)。
