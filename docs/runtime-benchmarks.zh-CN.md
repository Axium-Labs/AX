<a id="runtime-performance-measurement"></a>
# 运行时性能测量

[English](runtime-benchmarks.md) | **简体中文**

生产 Agent 任务执行见 [agent-benchmarks.md](agent-benchmarks.zh-CN.md)。下述 Phase 1 结果仍是离线运行时测量，不是能力分数。

AX-Bench Phase 1 是位于 [`benchmarks/runtime/`](../benchmarks/runtime/README.md) 的独立基准基础设施。它链接公开生产 API，不改变运行时行为或新增生产 target，测量离线 Release CLI 启动、本地 Session／Context／Memory 操作、工具注册／调度、子生命周期和有界 supervisor 并发。

模型、外部工具和 child-host 持久化边界采用确定性 mock，与仅本地生产路径分开标注。mock 与内核组合延迟不等于端到端 Agent 性能。上下文测量不宣称模型驱动压缩性能，本地恢复组件不代表整个 CLI 恢复流程。不提供能力评分或 CI 性能门槛。

runner 使用隔离 fixture，至少三轮独立进程测试，生成包含环境、源码标识、耗时统计和整个 worker CPU／工作集遥测的原始 JSON、CSV、Markdown。操作系统缓存状态不受控。精确单位、计时边界、离线前提、资源限制及跨版本对比见基准 README。
