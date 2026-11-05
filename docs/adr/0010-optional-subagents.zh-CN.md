<a id="0010-optional-model-selected-subagents"></a>
# 0010. 可选的模型自主子代理

[English](0010-optional-subagents.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

AX 已具有支持检查点的 Agent Loop、有界工具轮次调度器、AgentSupervisor、隔离 ChildHost、注册表重新绑定和审批策略。模型选择的委派必须复用这些机制，保留禁用行为和冷启动保证。

<a id="decision"></a>
## 决策

委派默认禁用。仅在启用的轮次注册一个 `subagent` 工具，调用运行时 spawn/wait 原语。准入有界，执行使用 ChildHost 与 AgentSupervisor::run_child。继承提供商及审批策略，仅以显式任务／上下文初始化，收窄重绑定注册表，并从工作分支移除委派。返回最终结果信封，发出紧凑生命周期事件。现有配置 JSON/TOML 迁移和 CLI/TUI 设置负责持久化，每轮前重新加载。取消停止同一循环并关闭主机回执。

<a id="consequences"></a>
## 影响

无需额外规划器调用、强制任务拆分、并行运行时或子历史注入。禁用轮次不构建管理器或委派 schema。Host 工作区／会话／产物语义仍是权威：本地子工作区可丢弃，不合并回父工作区。深度限制为一层，有界准入避免单轮无限创建。
