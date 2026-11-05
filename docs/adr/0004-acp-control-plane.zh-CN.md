<a id="0004-acp-adapter-at-the-ax-cli-composition-boundary"></a>
# 0004. 在 AX CLI 组合边界提供 ACP 适配器

[English](0004-acp-control-plane.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

AX 完整轮次初始化位于 CLI `ReplState`，涵盖模型选择、会话恢复、记忆与技能上下文、MCP 网关、权限和检查点。内核暴露可复用循环及事件，但新控制面需要会话级命令和远程设备路由，同时避免复制初始化逻辑。

<a id="decision"></a>
## 决策

在 AX CLI 中增加 ACP v1 stdio 适配器及主动连接的 Crew 设备桥接。适配器调用现有 `ReplState` 和 `run_prompt_with` 路径，通过自身 `ApprovalPolicy` 向客户端转发权限请求。取消会终止当前轮次，恢复时使用 AX 现有的中断工具恢复机制。Crew 位于独立 Rust 后端，保存编排元数据、最终任务输出及脱敏事件索引；AX 保留完整会话、消息、记忆、工具、技能、MCP、模型和凭据。

<a id="consequences"></a>
## 影响

内核与存储 crate 不变。现有 `ax run`、`ax agents` 和 TUI 路径保留原行为。适配器目前要求进程工作目录等于 ACP 会话 `cwd`，因此 Crew 在各成员目录分别启动 AX 进程。取消无法撤销已完成工具副作用；恢复只标记中断调用，不重新执行。多个独立 Crew 任务可经独立 ACP 进程并行，确定性依赖仍由 Crew 负责。

<a id="workspace-discovery-extension"></a>
## 工作区发现扩展

_ax/workspace 为只读接口：可选 cwd 指定规范化的已有目录，省略则从用户主目录开始。返回 cwd、parent 及带 name/path 的 directories，仅列目录。缺失路径或非目录返回无效参数。严格沙盒将规范化 cwd、列出目标（包括符号链接）和 parent 限制在进程工作区内。_ax/capabilities.workspace 声明该方法及其只读属性。

Crew 心跳在 capabilities.workspaces 中包含已注册 AX 根目录的 id/name/path，使项目选择无需放松“执行必须打开已注册根目录”的桥接要求。会话 cwd 匹配规则不变。
