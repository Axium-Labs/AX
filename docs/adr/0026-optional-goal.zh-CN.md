<a id="0026-optional-goal-in-the-existing-execution-kernel"></a>
# 0026. 现有执行内核中的可选 Goal

[English](0026-optional-goal.md) | **简体中文**

状态：已接受

<a id="decision"></a>
## 决策

将原停止验证器就地演进为 Goal，保留单一 GoalEvaluator 边界与现有 Agent Loop；绝不新增 Goal 分发器、数据库或并行验证器。任务队列仍是唯一标识／状态检查点。普通轮次默认不带 Goal，保留直接流式完成。

全局／项目设置是显式目标创建或主动启用普通新任务自动 Goal 接纳的模板。主机 `enabled` 偏好默认 false，显式 Goal 生命周期和传入 worker 策略保持独立。CLI/TUI/ACP 共用现有 Pursue 意图进行接纳，保留调用方拥有的任务 ID。旧名称在配置／历史边界转换。验收只限当前目标，worker 不继承验证策略。worker 指标来自既有子回执，有独立预算，不继承目标完成策略。轮次／时间上限默认无限，约束既有 ExecutionBudget／超时路径。Token／费用上限需要完整报告统计，估算不执行硬上限。Harness 进度推进驱动可配置停滞处理，重试由既有 RetryPolicy 负责。

配置的 Command、HTTP 和 host/MCP 验收适配器包装 GoalEvaluator，使用现有 Scheduler、工具沙盒、权限和 Session 证据接收端。规则、版本、绑定及不安全在途副作用随 Goal 持久化。保守停滞判断纳入实测外部／子任务证据，将外部等待挂起，需主机／用户显式恢复。可选 RuntimeExtension 检查点复用 Crew 带所有权隔离的 Workflow CAS，保存累计消耗及有界回执；无可选 Goal 的普通任务不产生这些调用。本地 Session 租约保护执行与恢复写入。验证器缺失或本地副作用未协调时禁止自动接管；不新增预算账本或分布式调度器。

Goal 以 completed、paused、blocked、failed 或 cancelled 区分结束，未知证明不能变为成功。推理前持久化保守预留，推进前保存结构化工具证据；恢复绝不自动重放未知外部副作用。AXCrew 继续负责分布式所有权和调度。

<a id="consequences"></a>
## 影响

除非用户启用作用域 Goal 模板，普通提示不额外发审查请求或 Token 估算请求。有限上限和基于证据的停滞处理约束可选续接。硬中断可能高估显示统计及耗时，用户检查记录后显式调整上限／恢复。内核并非总能确定外部副作用和第三方工具模型费用。见 [goal.md](../goal.zh-CN.md)。
