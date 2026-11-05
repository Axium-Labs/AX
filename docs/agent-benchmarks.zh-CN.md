<a id="agent-capability-and-advanced-benchmarks"></a>
# Agent 能力与高级基准测试

[English](agent-benchmarks.md) | **简体中文**

[AX-Bench Agent](../benchmarks/agent/README.md) 使用生产 CLI 执行能力测试和 Advanced 控制器，不改变 AX/AXCrew 运行时逻辑。标准套件有 60 个确定性自定义任务；24 场景 Advanced 目录分发长上下文／长阶段任务、压缩／轮换断言、崩溃／Session 恢复、实际 API/MCP 传输故障、原生约束检查、隔离安全向量和真实 Crew 实验。每任务重复至少三次。

执行器保留轨迹及失败证据，使用真实已配置 Provider，应用任务／全局预算，输出包含源码、模型、环境和数据集标识的 JSON/CSV/Markdown 报告。环境缺失记为 `not_run`；已执行但缺完整计费或官方分数的测试明确记为 `unscorable`/`ungraded`，排除在计分成功率分母之外。离线计划或控制器测试不构成 Agent 能力结果。

独立可执行适配器覆盖受限 Docker 安全测试；真实本地／SSH worker 生命周期、故障／恢复、DAG／产物／一致性检查和只读资源采集；以及固定版本的 SWE-bench/Terminal-Bench 预备环境及官方确定性验证器。物理机器标识仍由操作者证明，不能根据多个逻辑 Host ID 推断。包 README 及关联适配器文档说明精确范围和前提。

原生 Token／费用上限需要完整 Provider 使用量／计费，在途请求可能越过上限。未知费用不是零。传输重连不证明物理网络恢复；声明攻击向量不代表普遍安全。远程和容器资源测量明确保留采样及归因边界。真实模型运行前不存在已知分数。
