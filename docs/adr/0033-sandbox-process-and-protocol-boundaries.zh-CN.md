<a id="0033-sandbox-process-protocol-and-cache-boundaries"></a>
# 0033. 沙盒进程、协议与缓存边界

[English](0033-sandbox-process-and-protocol-boundaries.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

错误字符串混淆普通命令失败和隔离失败。工具／MCP 共用 PID 命名空间，序列化命令 argv 暴露服务秘密。只终止直接子进程会留下后台任务，同步安全查询还会准备命名空间。

<a id="decision"></a>
## 决策

结构化标记 worker 结果。对象授权、隔离或 worker 启动失败为 SandboxViolation，普通工具失败仍为 Execution。工具 broker 限定工作区；各 stdio MCP 传输分配独立 broker/PID 命名空间。经私有 socket 发送有界帧，独立于子进程 stdin。取消和退出时终止命令进程组，保留 Linux 信号码。阻塞 worker 准备会话工具命名空间，缓存成功结果，失效 broker 仅重建一次，不重放命令或回退主机执行。

Strict 强制禁止联网，workspace 保留自动审批和联网。使用 RLIMIT_DATA 及按 CPU 缩放的 RLIMIT_NPROC，每检测 CPU 为 256，无需特权 cgroup。挂载 AX_HOME 下、工作区之外以工作区为键的缓存、运行时资产及生成的当前 uid/gid 标识。

<a id="consequences"></a>
## 影响

Workspace 模式不能阻止工作区数据外泄。生成目录树按发现跳过规则筛查硬链接，仍视为可信输入。进程组覆盖普通任务，不覆盖蓄意 setsid 逃逸；命名空间销毁仍是最终生命周期边界。RLIMIT_DATA 既非 RSS 配额，也非聚合配额。MCP 命名空间共享工作区／缓存文件，不共享进程。Off 保留主机执行语义。精确边界见 [security.md](../security.zh-CN.md)。
