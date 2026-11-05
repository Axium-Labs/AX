<a id="0028---native-desktop-adapters"></a>
# 0028 - 原生桌面适配器

[English](0028-native-desktop-adapters.md) | **简体中文**

日期：2026-10-14

<a id="decision"></a>
## 决策

保留统一桌面工具 schema、动作审批策略和独立应用权限存储。Windows 保留 Rust UIA 驱动，Unix 使用内嵌、一次性的 Python 适配器，在同一 Rust 主机传输后接入 Linux AT-SPI 及 macOS Accessibility/Quartz/ScreenCaptureKit。

运行时保持惰性，不安装 Python 依赖或授予系统隐私权限。产生副作用前解析原生 helper 配置及当前应用标识；Unix 原生发现后再次检查持久拒绝。临时驱动代码为私有文件。专用进程组在取消时一并停止 helper 和驱动。worker 正常完成或自身超时时清理脚本，突然取消可能留下不活跃私有文件。Unix 操作截止时间短于外层原生 worker。不接受模型任意指定程序或脚本。

<a id="boundaries"></a>
## 边界

图形会话／平台支持、已安装驱动就绪及 macOS TCC 授权分别判断。Linux 原始输入／捕获需要可验证的 X11 窗口归属及前台／遮挡检查。Wayland 目前仅用无障碍读取和动作。遇到密码、不可读或不完整 UI 扫描时不提供截图；目标窗口捕获绝不变为全桌面截图。

这些主机适配器不能补足缺失的原生工作区沙盒。WSL 可操作 Linux GUI 应用，不能操作 Windows 应用；SSH 不暴露本地 UI。

<a id="validation"></a>
## 验证

Windows 和隔离 Linux X11 fixture 验收调用实际 UI API。macOS 隐私／按键路由有隔离适配器测试，但仍需原生框架／TCC 验收及真实 Wayland 合成器验收。
