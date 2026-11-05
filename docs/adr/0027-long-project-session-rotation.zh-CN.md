<a id="adr-0027-optional-working-context-and-session-rotation"></a>
# ADR 0027：可选工作上下文与 Session 轮换

[English](0027-long-project-session-rotation.md) | **简体中文**

状态：已接受

长期 Goal 需要有界模型上下文及关联证据的交接，不能引入第二份任务、记忆或执行存储。

使用可选 LongProjectHost 接点，为每个 Goal 提供 Markdown 投影。标识、标准、累计消耗和执行状态仍在现有 Goal/Task Queue 检查点。默认关闭，仅在显式选择 Long Project 或单独配置轮换策略后启用轮换。

在安全模型边界评估上下文压力及实测 Compaction 收益。先初始化普通后继 Session，再在旧 JSONL 流提交重定向；只恢复已提交后继。交接中保留内核与权限、原始历史及现有恢复屏障。交接失败保留前驱，不能允许完成或自动重放工具。

按参与者分割受管理文档块，通过内容摘要、替换前比较及操作系统文档租约保护人工编辑。归档旧上下文，绝不把文字当权威任务状态。现有带所有权隔离的 Crew Workflow 检查点可携带文档和摘要以供接管，主机所有权及协调规则不变。

配置、命令、恢复、权限和限制见 [Long Project](../long-project.zh-CN.md)。不新增调度器、周期模型调用、独立持久化服务或可复用记忆存储。
