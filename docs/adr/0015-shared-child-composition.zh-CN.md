<a id="0015-shared-child-composition-and-explicit-complete-task-inputs"></a>
# 0015. 共用子执行组合与显式完整任务输入

[English](0015-shared-child-composition.md) | **简体中文**

状态：已接受；扩展 0008

<a id="context"></a>
## 背景

ACP 命令分发早于交互初始化，难以审查前端接线是否一致。当前 ACP 适配器已使用公共提示 runner，但缺少通过该适配器实际执行子任务的测试。旧版从列表派生计划，以及一律禁止非空队列重规划，可能将基准规则误作工作，并阻止任何任务执行前的显式重规划。

<a id="decision"></a>
## 决策

保留单一 Agent Loop 和共用提示组合边界。CLI、TUI、ACP 经同一 helper 和构造器绑定 LocalChildHost、项目作用域和两种执行预算。使用真实 LocalChildHost 测试每个提示适配器，不用假主机或强制提示指令。

只有显式 task_queue start 创建可执行任务。为兼容性接受完整任务输入字符串，也接受 title/input 对象以区分展示标题。子代理收到完整输入，控制器进度包含输入。动态实例枚举发生于普通数据读取及显式 start 之后。

同一 Goal 的活跃队列尚无已分发任务、回执或结果时可替换；归档旧状态，不删除原始历史。分发／配置前保存 execution_started 检查点，保守保留未知在途副作用。已执行或终态工作必须以显式新 Goal 取代。旧字符串检查点将已存标题作为完整输入。

<a id="consequences"></a>
## 影响

Crew 与 CLI/TUI 获得相同隔离子会话、顺序队列消费、独立子预算和失败后续接。不引入额外规划器、Agent Loop、权限绕过或列表启发式。运行时验证输入存在性，语义完整性仍由模型／调用方负责。
