<a id="architecture-decision-records"></a>
# 架构决策记录

[English](README.md) | **简体中文**

ADR 记录具有长期影响的设计决策：背景、决策和接受的权衡，使代码背后的*原因*可以查阅。

<a id="index"></a>
## 索引

| ADR | 标题 | 状态 |
|---|---|---|
| [0001-memory-scopes.md](0001-memory-scopes.zh-CN.md) | 具有可移植项目标识的作用域记忆 | 已接受 |
| [0002-session-event-log.md](0002-session-event-log.zh-CN.md) | SQLite 索引与 JSONL 会话事件日志 | 已接受 |
| [0003-portable-axpack.md](0003-portable-axpack.zh-CN.md) | 带版本的可移植 AX 数据包 | 已接受 |
| [0004-acp-control-plane.md](0004-acp-control-plane.zh-CN.md) | 在 AX CLI 组合边界提供 ACP 适配器 | 已接受 |
| [0005-tool-round-scheduler.md](0005-tool-round-scheduler.zh-CN.md) | 识别依赖关系的工具轮次调度 | 已接受 |
| [0006-skill-evolution.md](0006-skill-evolution.zh-CN.md) | 在运行时内核之外进行技能进化 | 已接受 |
| [0007-installation-storage.md](0007-installation-storage.zh-CN.md) | 安装目录拥有的持久状态 | 已接受 |

- [0008 — 持久化轻量任务队列](0008-durable-task-queue.zh-CN.md)
- [0009 — 绑定 Goal 的执行不变量](0009-execution-invariants.zh-CN.md)
- [0010 — 可选的模型自主子代理](0010-optional-subagents.zh-CN.md)
  （深度／默认行为已被 [0023](0023-scoped-subagent-depth.zh-CN.md) 取代）。

<a id="adding-an-adr"></a>
## 新增 ADR

1. 将下方模板复制到 `adr/NNNN-short-title.md`，使用下一个编号。
2. 保持简短：背景 → 决策 → 影响。
3. 更新上方索引。

<a id="template"></a>
### 模板

```markdown
# NNNN. Short title

Status: proposed | accepted | superseded by NNNN

## Context

The problem and the constraints that matter.

## Decision

What we chose to do.

## Consequences

What becomes easier, what becomes harder, and what we accepted.
```

- [0011: 作用域能力](0011-scoped-capabilities.zh-CN.md) — Skill、MCP 和 Agent 管理共用 Global/Project 注册表。
- [0012 — 分层记忆检索](0012-layered-memory-retrieval.zh-CN.md)
- [0013 — 工作区运行时沙盒](0013-workspace-runtime-sandbox.zh-CN.md)
- [0014 — 建议性执行策略](0014-advisory-execution-policy.zh-CN.md)
- [0015 — 共用子执行组合](0015-shared-child-composition.zh-CN.md)
- [0016 — 并行子任务分发、结构化回执与可解析指令](0016-parallel-children-and-instructions.zh-CN.md)

- [0012 — 分层本地记忆检索](0012-layered-memory-retrieval.zh-CN.md)

- [0011: 模型自主策略](0011-model-directed-policies.zh-CN.md) — 语义选择受确定性运行时上限约束。

- [0013: 工作区运行时沙盒](0013-workspace-runtime-sandbox.zh-CN.md) — 独立于 Permission 的中央操作系统隔离。

- [0014: 建议性执行策略](0014-advisory-execution-policy.zh-CN.md) — 观测和恢复不阻断安全工具执行。

- [0015: 共用子执行组合](0015-shared-child-composition.zh-CN.md) — 前端一致性、显式完整任务输入及未执行计划替换。

- [0017 — 权威 Experience JSONL](0017-evolution-jsonl-authority.zh-CN.md) — Evolution 控制检查点、增量消费和旧格式迁移。

- [0018 — 续接状态驱动完成](0018-continuation-driven-completion.zh-CN.md) — 无强制最终审查器，Goal 可选。

- [0019 — 搜索提供商路由](0019-search-provider-routing.zh-CN.md) — 提供商专属传输、可达性／延迟路由、熔断和有界对冲。

- [0020 — 原生模型提供商](0020-native-model-providers.zh-CN.md) — 原生协议／认证、共用分发、签名回放及独立资源配置状态。

- [0021 — 持久化分布式协作](0021-durable-distributed-collaboration.zh-CN.md) — 可选 CLI worker、带隔离的所有权及基于检查点恢复。

- [0022 — 惰性会话 Mod](0022-lazy-session-mods.zh-CN.md) — 带作用域元数据、会话拥有的 Node 状态及明确隔离限制的可执行扩展。

- [0024 — 独立主机访问](0024-independent-host-access.zh-CN.md) — 工作区隔离、应用／网站访问和动作审批分离。

- [0025 — 原生优先托管搜索](0025-native-first-hosted-search.zh-CN.md) — 实际模型协议及已授权匿名 MCP 回退。

- [0026 - 可选 Goal](0026-optional-goal.zh-CN.md) - 现有循环和队列中的目标作用域验收。

- [0027 — 可选 Long Project 与 Session 轮换](0027-long-project-session-rotation.zh-CN.md) — 现有 Goal/Session 持久化中的证据投影与已提交交接。

- [0028 - 原生桌面适配器](0028-native-desktop-adapters.zh-CN.md) - Windows、Linux、macOS 原生驱动共用应用／动作权限。

- [0029 - 基于证据的交接](0029-evidence-driven-handoff.zh-CN.md) - 提问和普通最终响应前进行有界错误恢复及报告自审。

- [0030 - 统一模型自主委派](0030-unified-model-selected-delegation.zh-CN.md) - 不依赖任务类型的模型决策，共享本地并发、深度及权限上限。

- [0031 - 工具声明逐字输出](0031-tool-declared-verbatim-output.zh-CN.md) - 各次工具执行声明无损源码语义及识别行的上下文投影。

- [0032 - 经验证的补丁与有界回执](0032-verified-patches-and-bounded-receipts.zh-CN.md) - 强制补丁预期内容、最终坐标区域、简短重复引用及 FIFO 字节有界输出保留。

- [0033 - 沙盒进程、协议与缓存边界](0033-sandbox-process-and-protocol-boundaries.zh-CN.md) - 类型化失败、独立 MCP 命名空间、进程组清理和外部持久缓存。

- [0034 - 需审核的 Evolution 晋升与有界分析恢复](0034-reviewed-evolution-and-analysis-recovery.zh-CN.md) - 显式候选审核、持久化失败退避及包回滚。
