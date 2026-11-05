<a id="0024--separate-workspace-confinement-from-host-ui-access"></a>
# 0024 — 将工作区限制与主机 UI 访问分离

[English](0024-independent-host-access.md) | **简体中文**

状态：已接受，2026-10-10。

此前桌面操作要求关闭沙盒，并使用通用 Process 权限；浏览器依赖外部 CLI，其会话与重定向行为无法可靠执行 AX 网站授权。工作区访问、应用／网站访问，以及某次具体操作审批必须独立决策。

在现有 Tool 和 ApprovalPolicy 契约中添加 `ExecutionBoundary::AuthorizedHost` 及类型化主机访问请求／授权。文件、命令、Skill 和本地 MCP 仍由 WorkspaceWorker/SandboxManager 管理。应用标识从目标进程可执行文件解析，网站按精确 HTTP(S) origin 识别；即使普通能力／profile 审批允许操作，仍需单次／会话／持久授权。未知审批实现默认拒绝主机访问。

持久规则独立于配置与工作区数据，存于 AX 主目录。临时访问和浏览器会话限定到现有控制器 PermissionStore。执行前重新检查标识及撤销。CLI、TUI、ACP 和 Mod 内置调用共用该边界；AXCrew 是设置／审批前端，不是另一权威。

浏览器由内嵌固定 Node/Playwright worker 管理，不使用模型可调用的外部 CLI。阻止未授权来源、各级重定向及不受控浏览器通道，不导入个人 profile。截图通过现有图像输出返回，不扩大文件系统权限。

不新增 Windows/macOS 操作系统沙盒后端或非 Windows 桌面驱动。当前浏览器策略阻止跨来源资源和 WebSocket，限制网站兼容性。原生扩展及不受限的同用户程序仍被信任。当前详细行为与限制见 [host-permissions.md](../host-permissions.zh-CN.md)。

修订，2026-10-19：已有 Computer Use 应用允许规则也满足该应用普通动作审批。先解析主机访问，再做通用提示，避免已保存／会话授权后每次点击仍询问。任务级 Ask/Deny 规则和能力 Deny 仍为权威，执行时重新检查标识及撤销。最小窗口发现是安全操作。浏览器动作保留原独立审批契约。此修订作用于调度器和 Mod 内置桥接，不改变已存授权格式。
