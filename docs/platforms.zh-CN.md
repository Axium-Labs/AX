<a id="desktop-platforms"></a>
# 桌面平台

[English](platforms.md) | **简体中文**

AX 以同一 Rust 工作区支持 Windows、Linux 和 macOS。CLI、TUI、ACP、提供商、会话、记忆和分布式 worker 共用运行时。平台分支负责 shell 进程、路径、凭据权限及可执行文件替换，不引入第二套代理运行时。

<a id="build-and-install"></a>
## 构建与安装

原生构建使用 `cargo build --release --locked`。Windows 使用 `ax.exe`，Linux/macOS 使用 `ax`。发布矩阵构建 Windows x64、Linux x64 GNU、Linux x64/arm64 musl 和 macOS Intel/Apple Silicon。Unix 归档含可执行 `ax`、`skills/` 及 `mcp.example.toml`，资产名匹配安装器和更新器。平台检查 CI 在 PR 或手动触发时运行原生构建、lint 和工作区测试；发布仍由 tag 触发。新资产只有包含它们的 release 实际发布后才可下载。

Unix 安装器默认安装到 `~/.local/bin`，支持 `AX_INSTALL_DIR` 并验证 `SHA256SUMS`，在目标旁暂存后原子重命名，AX 运行时也可更新。保留已有配置、凭据、会话、用户技能及 MCP 配置。与 Crew 共享状态需显式设定 `AX_HOME`：Crew 默认 `~/.ax`，独立 AX 默认 `<install-dir>/.ax`。

<a id="shell-settings"></a>
## Shell 设置

`ax environment --terminal-shell default|sh|bash|zsh|fish` 配置 Crew 的 Unix 集成终端。Unix 默认 `default`，Windows 默认 `powershell`。`/environment` 提供适合平台的选项；Windows 保留 PowerShell、Command Prompt、Git Bash 和 WSL。拒绝其他平台的 shell 选项。这些设置不改变代理 shell 工具的 POSIX sh／Windows PowerShell 契约。

Crew 从可执行 `$SHELL` 解析 `default`，回退到 macOS `/bin/zsh` 或 Linux `/bin/bash`，再回退 `/bin/sh`。显式 shell 必须已安装。导入的 Windows 终端偏好在 Unix 回退为默认，不改写已保存配置。WSL 仅限 Windows。

<a id="capability-limits"></a>
## 能力限制

原生 `desktop` 适配器使用 Windows UIA、Linux AT-SPI 和 macOS Accessibility。Unix 需文档指定 Python 运行时，macOS 还需 TCC 权限。Linux X11 支持原始输入／捕获，Wayland 只使用无障碍动作。macOS 原生桌面仍需 Mac 验收运行，见 [computer-use.md](computer-use.zh-CN.md)。NVIDIA NVML 遥测可选，不可用 GPU 读数保持不可用。

Workspace/strict 操作系统隔离需要现有 Linux 后端及 bubblewrap/prlimit。原生 Windows/macOS 隔离尚未实现，显式受限模式会拒绝运行，见 [security.md](security.zh-CN.md)。平台适配不放松沙盒或主机访问决策。

<a id="verification-status"></a>
## 验证状态

当前本地验证覆盖 Windows x64 和 Ubuntu/WSL Linux x64 原生工作区测试与 lint，以及可运行的 Linux x64 musl 二进制。原生 Computer Use 通过真实 Windows 和隔离 GTK/X11 fixture 验收，含独立应用权限拒绝及密码保护。Linux 使用虚拟 X server，不是物理桌面或 Wayland 合成器。macOS 及 Linux arm64 构建任务已配置，但未在本地环境执行。声称已验证支持或分发相应资产前，仍需其原生构建／运行时检查。
