<a id="0013-workspace-runtime-sandbox"></a>
# 0013：工作区运行时沙盒

[English](0013-workspace-runtime-sandbox.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

工具专属路径检查和权限提示无法约束 shell 间接执行、子进程或本地 MCP 服务。AX 需要无需 Docker 的操作系统边界，使工作区自主执行不受模型、Skill 和子代理选择影响。

<a id="decision"></a>
## 决策

在工具下方引入独立沙盒 crate 和中央管理器。Linux 使用持久 bubblewrap 命名空间、固定文件系统对象、seccomp 和 rlimit。内置文件系统工具在私有 worker 中运行，shell 与 stdio MCP 经同一执行器启动。未声明执行边界默认拒绝。受限模式在无法提供约束时拒绝运行，包括目前不支持的平台。权限保持独立层，不能静默削弱运行中的沙盒。

Crew 选择本地已注册工作区标识。子运行时使用私有 Git 克隆和收窄的配置能力。保留的管理器跨调用复用命名空间，销毁时终止后代进程。

<a id="consequences"></a>
## 影响

Agent Loop 保留既有 Permission 与工具编排。Linux 部署需要文档指定的内核和用户空间能力。Windows/macOS 必须有原生后端才能使用受限模式。拒绝未验证文件系统和含硬链接的工作区输入。可信运行时服务继续负责 AX 状态，任意工具副作用只经沙盒执行。

执行方式、限制和可运行测试见 [security.md](../security.zh-CN.md)。
