<a id="adr-0029-evidence-driven-handoff-after-recoverable-errors"></a>
# ADR 0029：可恢复错误后基于证据的交接

[English](0029-evidence-driven-handoff.md) | **简体中文**

状态：已接受，2026-10-18。

<a id="context"></a>
## 背景

一次发布执行将客户端未登录当作不可用外部前提，却未尝试文档记载的现有认证接口；还通过脚本替换引入错误版本行，将错误归因于原文件，并在修改项目版本后声称项目未变。现有建议性恢复策略未能在这些用户交接前触发审查。

<a id="sources"></a>
## 来源

- [OpenAI Codex 提示指南](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)：在用户范围内持续执行、使用专用编辑工具、从错误恢复并验证结果。
- [OpenAI Apply Patch](https://developers.openai.com/api/docs/guides/tools-apply-patch)：结构化文件变更和明确工具反馈。
- [Claude Code Stop hook](https://code.claude.com/docs/en/hooks#stop)：反馈可在停止前续接执行，有界续接避免重复停止循环。
- [Anthropic 有效 harness](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)：显式进度记录和结果验证减少过早完成。

这些公开实践启发了下述 AX 设计，但不能证明任一产品防止所有此类失败，或使用与 AX 完全相同的实现。

<a id="decision"></a>
## 决策

保留单一执行循环，不增加任务／文字分类器。加强通用策略：完成已授权流程、复用受支持的现有授权、修复自身编辑并报告实际状态。纠正无条件声称 shell 需要审批的指引；权限决策仍归现有运行时。

仅当前主机轮次实际发生的类型化可恢复工具错误触发有界交接检查点。第一个有效用户问题得到普通 `review_required` 结果，而不挂起。只有最新工具调用轮次仍有可恢复错误时，普通最终响应才触发一次同模型审查续接。后续操作／检查成功或队列结果已记录时直接完成，不重复审查旧失败。每个边界每主机轮次最多干预一次，不因新增错误增加次数。反馈最多引用八个近期失败调用 ID，不重复命令输出或凭据。模型可使用工具恢复、再次提出有证据支持且只能由用户回答的问题，或凭充分证据结束。不自动执行指定命令或认证绕过。

现有 Goal 验证拥有其最终边界。有限步骤预算耗尽时不发额外请求。类型化权限拒绝／全局阻塞、用户暂停／取消及排队／补充指令仍沿用已有处理。错误回执从当前轮次原始消息读取，压缩不能移除触发条件，旧轮次失败不能触发。待处理候选标记防止子任务恢复将中断草稿误作完成。

<a id="tradeoffs"></a>
## 权衡

有错误的主机轮次最多增加两次执行模型请求，成功直接轮次不增加。真实阻塞问题可能在展示给用户前审查一次。这是自我审查，不是独立验证；模型反馈后仍可能推理错误。流式输出可能先暴露候选再纠正，前端必须以 TurnFinished 作为完成信号。设计刻意避免发布专属凭据读取器、自动扩权、每次编辑后强制测试，以及无界停止守卫。
