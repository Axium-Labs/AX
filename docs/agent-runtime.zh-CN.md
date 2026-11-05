<a id="agent-runtime-request-execution-capabilities"></a>
# Agent 运行时：请求、执行与能力

[English](agent-runtime.md) | **简体中文**

AX 是 **coding execution harness**。当前用户请求定义工作，运行时推动所需交付物进入终态：完成、附证据失败，或等待只能由用户作出的决定。本文定义 CLI、TUI、ACP、Crew 桥接及隔离子执行共用的运行时契约，含请求边界、上下文和能力。[Agent Loop](agent-loop.zh-CN.md) 介绍轮次续接，[coding harness](coding-harness.zh-CN.md) 介绍任务执行、工作区准备及恢复。

普通任务队列用 Goal ID 标识身份，但该 ID 不会启用可选 [Goal](goal.zh-CN.md) 验收／续接策略；需要显式创建 Goal，或启用作用域自动 Goal 模板。Harness 与 Goal 共用此标识及单一执行循环。

<a id="core-contract"></a>
## 核心契约

> 用户请求定义工作；上下文支持工作；工具提供能力；执行持续到所需结果进入终态。

- **请求定义工作。**工作区内容、会话历史、已有文件和项目指令是*支持上下文*，可能相关，但其存在绝不扩大请求。
- **跟踪执行。**每个 Goal 在活跃任务队列（`begin_goal`）上运行。队列项是具体用户交付物，不是发现清单；跟踪工作进入终态才结束轮次。
- **工具提供能力。**能力指引描述*被选中后*如何使用，不意味着模型应选择它。
- **没有模式开关。**不存在关闭 harness 的配置标志或 builder 方法。Host 注册哪些工具和提供商这一组合方式，是唯一变化点。

<a id="the-always-on-prompt-and-context"></a>
## 始终启用的提示与上下文

每次执行由内核在 `prepare_environment` 注入同样有界提示和运行时上下文：

- `[ax-agent-runtime]`：身份、请求／上下文边界、最小充分操作规则及权限边界，位于 `crates/core/src/runtime_core.rs`。
- `[ax-capability-guidance]`：按工具名确定顺序，由各注册工具经 `tool::Tool::guidance` 声明的指引组装。
- `[ax-environment]`：完整环境快照，含 cwd、工作区根、沙盒及网络状态、shell 契约和缓存可执行探测，由 `EnvironmentContext::detect` 提供。
- `[ax-coding-harness]`：执行策略 `crates/core/src/harness.rs::POLICY`，含可恢复初始化观测、模型选择委派、队列纪律、子输入契约和终态要求。
- `[ax-delegation-availability]`：有效深度／并发上限，以及普通委派和队列子执行的结构可用性，每轮替换，不授予权限。

适用工作区指令（`AGENTS.md`、`.ax/rules`）由 CLI 提示边界每轮通过 `project_instructions::install` 注入独立上下文槽。没有全局“工具使用策略”提示。

<a id="one-turn"></a>
## 单个轮次

```text
user request
→ goal admission (every goal gets an active task queue)
→ inject [ax-agent-runtime] + [ax-capability-guidance] + [ax-environment] + [ax-coding-harness]
→ model → execute tools/tasks/children → append results → model
→ advisory loop-hygiene reminder on repeated identical calls
→ bounded handoff self-review after current-turn recoverable tool errors
→ queue terminal and no continuation → turn ends
```

只有一个 Agent Loop，没有硬编码的简单／检索／编码／长任务循环，不额外调用模型分类意图。

类型化可恢复工具错误后，第一个问题，以及最新工具轮次仍失败的普通最终响应，各最多触发一次同模型证据审查。恢复成功仍直接完成，使升级给用户前可先执行文档化恢复，并核对报告与真实变更。成功普通轮次仍直接完成；权限、用户显式暂停／取消和可选 Goal 评估仍为权威。见[工具错误后的证据审查](coding-harness.zh-CN.md#evidence-review-after-tool-errors)。

<a id="guidance-while-a-turn-is-running"></a>
### 执行中的补充指令

TUI 输入框在执行中仍可编辑。Enter 将普通文本作为指令加入当前执行，不取消模型流、不重启工具、不新建 Goal。内核在下一模型／工具边界按到达顺序消费，保存为用户消息，并以同一 Goal 和已完成结果继续。执行中仍可用 `/goal status` 及活跃 Goal 暂停／取消；其他斜杠命令排队到工作结束。最终准入边界后到达的提交作为普通下一轮。

嵌入方使用 `TurnInput::try_steer` 及 `AgentKernel::with_turn_input` 或 `set_turn_input`，返回消息 ID 表示已接受。关闭轮次和接受输入共用锁，成功最终响应不会遗留已接受消息。已完成／取消句柄保持关闭。补充指令不改变当前模型、推理强度或权限策略。

ACP 在同一连接为活跃会话提供 `_ax/steer`，接受 `sessionId` 和常规 `prompt` 内容块，返回 `{accepted:true,messageId}`。错误／已结束会话报错。实时及回放 `user_message_chunk` 带 `_ax.steering:true` 和稳定 ID，历史将指令保留在原轮次内。`_ax/capabilities.steering` 声明扩展，`session/cancel` 仍独立。

<a id="trigger-conditions"></a>
## 触发条件

| 机制 | 使用时机 | 不使用时机 |
|---|---|---|
| **上下文**（环境、工作区、记忆、会话、项目指令） | 与当前请求相关 | 仅其存在从来不是行动理由 |
| **工具** | 请求需要该能力 | 模型可直接满足请求 |
| **任务队列** | 多项或长期请求 | 请求可直接完成，空 Goal 队列随答案完成 |
| **子代理** | 有界子问题可从聚焦上下文、独立检查或并行推进中获益，足以抵偿协调开销 | 拆分小型或紧耦合工作只增加开销，或用户要求独立工作 |
| **request_user_input** | 只能由用户决定的事项需挂起 Goal | 权限问题，危险操作由权限系统授权 |

上下文默认注入；*有条件*的是模型是否据此行动，这由请求决定，绝非上下文本身存在与否。

主模型为研究、写作、文档审查、分析、规划和软件工作作委派决定，不由职业、前后端拆分、关键词分类器或复杂度阈值决定。策略鼓励有益委派，不要求用户点角色名；整合及最终答案由主代理负责。用户顺序和独立工作要求仍为权威。Agent 描述帮助选择合适已配置代理，聚焦临时任务无需命名定义。

普通子代理和显式队列子任务共用并发池及绝对深度预算，默认 8 / 1；深度 0 同时禁用。运行时检查覆盖准入、分发及恢复工作，子代理继承权限上限和工具限制。类型化工作清单只注册任务，不选择委派。见[工具](tools.zh-CN.md#subagents)和 [ADR 0030](adr/0030-unified-model-selected-delegation.zh-CN.md)。

<a id="loop-hygiene"></a>
## 循环卫生

`crates/core/src/loop_hygiene.rs` 检测连续相同调用，在第 3、5、8 次各发一次建议性 `[ax-loop-hygiene]` 提醒，要求检查前次结果、改变方法或认定任务已满足。绝不阻断，也没有全局工具调用上限，长期有效任务仍可大量调用。

<a id="global-stops-are-evidenced"></a>
## 全局停止需有证据

模型不能仅声明全局阻塞就结束 Goal。`task_queue block` 仅在引用调用 ID 的结果带有真实 `ToolResult.global_blocker`，即运行时分类 `ToolError::GlobalBlocked` 时接受。文字、未验证资源假设或局部失败被拒绝，模型需恢复或继续。

<a id="safety-is-not-behaviour"></a>
## 安全不同于行为

权限、审批和沙盒由运行时独立于提示执行，只授权操作，不定义行为，也不信任提示来实现强制约束。[权限层](tools.zh-CN.md)及[安全层](security.zh-CN.md)与行为提示层分离。模型声明步骤子作用域仅为建议，工具始终执行工作区边界，错误子作用域不会锁住恢复。

<a id="behaviour-tests"></a>
## 行为测试

`test/harness/runtime_neutrality.rs` 覆盖验收行为：每次运行携带 harness 上下文；普通问题直接回答且 Goal 队列完成；检索请求从已有知识回答或上网取来源，两者都不扫描工作区；cwd 文件不能劫持请求，运行时只报告位置不列内容；显式调查及修复验证执行至完成；长请求创建可恢复队列；重复调用在 3/5/8 次提醒而非阻止；真实全局阻塞无需摘要模型调用即可结束 Goal。
