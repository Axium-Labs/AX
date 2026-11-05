<a id="0018-continuation-driven-completion"></a>
# 0018. 由续接状态驱动完成

[English](0018-continuation-driven-completion.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

coding harness 曾对每条纯文本最终响应发起第二次模型审查，并暂扣流式文本，增加普通回答延迟，也混淆了实际工具／任务续接和可选验证。

<a id="decision"></a>
## 决策

所有入口共用 `TurnState` 和 `TurnContinuation`。最终响应且无待处理运行时工作时直接完成。等待状态等待执行或输入，不轮询审查器。任务分类器、文字启发式和历史工具使用标记均不参与完成判定。

流式输出立即发送。可选验证已在原实现上演进为默认不存在的 Goal。确定性检查先于明确选定的模型验证。GoalEvaluator 边界提供具体续接原因；标准仅属于当前 Goal，绝不由 worker 继承。执行请求与 Goal 验证请求分别报告，见 ADR 0026。

<a id="consequences"></a>
## 影响

普通轮次理论最低为一次模型请求、零审查请求。模型负责的规划仍归模型，内核无法证明所有隐含交付物都已识别。需要更强完成条件的用户可主动配置具体验证规则。旧审计记录仍可恢复，无需重建强制审查阶段。

运行时与配置契约见 [agent-loop.md](../agent-loop.zh-CN.md)。
