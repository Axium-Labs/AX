<a id="0021-durable-distributed-collaboration-at-the-cli-boundary"></a>
# 0021. CLI 边界的持久化分布式协作

[English](0021-durable-distributed-collaboration.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

多个独立 AX 实例需要跨 Host 协作，不替换现有循环、本地任务、会话、记忆、子代理或 Crew 设备桥接。最初规划工作流的 AX 可能消失，而远程工作继续。

<a id="decision"></a>
## 决策

在 CLI 组合边界添加主动拉取 worker 和可选工具，使用现有 ACP 在隔离的每次尝试工作区中执行。AXCrew 是持久 Task、Event、Artifact、Workflow State、放置、租约和重试的唯一权威。调度匹配 AX 能力及 Host 共享资源。所有写入以 incarnation/generation/lease 标识隔离旧所有者。所有者替换时复用父作用域稳定委派 ID。只保存选定检查点和结果，不复制完整模型 Context、Session 或 Memory。

<a id="consequences"></a>
## 影响

单机启动和 runtime-core 不变。工作流恢复不需要永久 Coordinator Agent 或高频对等通信。本地模型、提供商及工具配置保持独立。恢复从持久输入和检查点启动新 ACP 尝试，不恢复每项本地执行细节。至少一次执行要求外部副作用幂等。SQLite 提供单一控制面权威；复制共识和对象存储是未来部署扩展，不是当前保证。

<a id="connected-instance-configuration-and-inventory"></a>
## 已连接实例配置与资产清单

注册先建立标识和工作区策略。主动连接的 worker 通过原生探测在有界后台截止时间内报告 Host 硬件，使租约续期独立于缓慢清单采集。AXCrew 维护 Host 共享容量、未知检测字段和上次接受报告时间。已连接实例设置独立暂存，不混同于 worker 报告的活跃能力；本地配置、依赖安装和权限仍在 AX。不复制完整代理状态或本地凭据。
