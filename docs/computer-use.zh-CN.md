<a id="computer-use"></a>
# Computer Use

[English](computer-use.md) | **简体中文**

Computer Use 是默认开启的原生主机能力。Windows 使用 UI Automation，Linux 使用 AT-SPI，macOS 使用 Accessibility，可读取应用窗口并操作暴露的控件。Linux 需图形桌面会话；WSL 可操作 Linux GUI，但无 Windows 桌面桥接。SSH 不暴露本地桌面工具。桌面访问用独立授权主机传输，不走文件／终端 worker，不再要求关闭沙盒。这不增加原生 Windows OS 隔离，其显式 workspace/strict 仍拒绝执行。AX 启动不启驱动；批准调用后才启动 30 秒超时的一次性原生 worker。

Unix worker 总截止 20 秒，主机用私有进程组在取消／超时时一并停止 helper 和 Python 驱动。正常完成或自身超时删除私有临时代码目录，主机突然取消可能留下不活跃脚本。驱动为内嵌生产资源，不是本地测试文件；Windows 保留 Rust 原生 worker，无需 Python。

<a id="platform-dependencies-and-os-permissions"></a>
## 平台依赖与系统权限

Linux 需要 Python 3、PyGObject 和 AT-SPI typelib。Ubuntu/Debian：

```sh
sudo apt-get install python3-gi gir1.2-atspi-2.0 python3-xlib python3-pil
```

图形会话须启用 AT-SPI，应用须暴露无障碍元数据。X11 输入／捕获还需 XTest、XRes PID 验证及 EWMH 窗口管理器。Wayland 目前支持无障碍读取、控件动作和 EditableText，明确不提供原始键鼠或捕获，保守 XWayland 路径也如此。不使用无作用域合成器或全桌面捕获回退。

macOS 需要 Python 3 及 PyObjC ApplicationServices、Quartz、Cocoa、ScreenCaptureKit。显式将 `AX_HOME` 指向所需 AX/Crew 主目录后执行：

```sh
python3 -m venv "$AX_HOME/desktop/venv"
"$AX_HOME/desktop/venv/bin/python" -m pip install pyobjc-framework-ApplicationServices pyobjc-framework-Quartz pyobjc-framework-Cocoa pyobjc-framework-ScreenCaptureKit
```

在系统设置 → 隐私与安全性，向 macOS 显示的责任启动上下文（终端、AXCrew 或 Python）授予辅助功能权限，图像还需屏幕录制。API 拒绝仍报错，驱动不绕过 TCC 或自动授权。macOS 14+ 使用目标窗口 ScreenCaptureKit 截图，旧系统仅用指定 Quartz 窗口。

可信 `AX_DESKTOP_PYTHON` 指定解释器，否则 Unix 依次尝试 `<AX_HOME>/desktop/venv/bin/python`、PATH 上 `python3`。隔离 Python 加载内嵌适配器与已安装依赖，模型不能选脚本、解释器或任意代码。缺依赖返回可执行指引，不由工具安装。

<a id="configuration"></a>
## 配置

AX 在 `AX_HOME/config.json` 中管理 `computer_use`，默认安装目录 `.ax`。AXCrew Computer Use 页用类型化本地 `ax_computer_use` Tauri 命令及当前 AX CLI，无需工作区、提供商或活跃会话。Crew 传已有全局 `AX_HOME`，显式环境覆盖优先，否则 `~/.ax`，与本地聊天运行时共用。独立 AX 未设 `AX_HOME` 时仍默认安装主目录。

```sh
ax computer-use
ax computer-use --enabled true
ax computer-use --include-screenshot false --max-nodes 1200 --screenshot-width 1280
ax computer-use --enabled false
```

默认 `enabled=true`、`include_screenshot=true`、`max_nodes=1200`、`screenshot_width=1280`。节点数接受 1–10000，截图长边 320–4096 像素。无效编辑写前失败。输出含 `settings`、平台／环境 `supported`、`backend`、`requirements`、`limits`。`supported` 标识原生后端／会话，不探测依赖或 TCC，实际可用性在调用时检查。读取不创建配置；写入原子保存，保留其他配置、凭据和会话。AXCrew 主机确认后保存开关，数字失焦／Enter 保存。读失败可重试，写失败保留确认值。

缺失 `computer_use` 或 `enabled` 使用开启默认，已有显式 `enabled=false` 保持关闭，更新不覆盖。仍需应用访问；已有应用允许涵盖普通计算机动作，避免重复通用审批，显式任务 Ask/Deny 和能力 Deny 仍生效。最小窗口发现不提示。不支持平台／环境仍不暴露 desktop 工具。

构建轮次注册表时捕获设置。关闭则模型没有桌面工具；开启／增加限制在下次注册表构建生效。各 helper 还重读主机设置：禁用拒绝已有注册表后续调用，限制和截图取捕获值与当前值较严格者。桌面动作不能写设置或启用访问。这是能力门槛，不是隔离同用户任意程序的 OS 沙盒；主机 shell 权限独立。

<a id="actions"></a>
## 动作

- `list_windows`：返回可见顶层窗口，含不透明平台 ID、进程 ID 和验证的可执行身份；隐藏拒绝应用，持久允许前不显示标题。可选 `include_hidden` 包含屏幕外窗口。
- `read_window`：需 `window_id`，扁平 `ui_tree.nodes` 含 ID、父 ID、角色、名称、可选值、边界、焦点和启用状态。可选 `max_nodes` 只能降低配置上限，`truncated` 表示省略；硬深度上限避免异常遍历。
- `screenshot_window`：经设置和隐私检查返回目标窗口 PNG，不提供全屏捕获。
- `focus_window`：明确切换焦点，拒绝会报告。
- `click_element`：使用 Invoke、Toggle、SelectionItem 或 ExpandCollapse 模式，需 `window_id` 及新读取的 `element_id`。
- `type_text`：传 `window_id`、`text`、可选 `element_id`；省略时仅使用属于目标的焦点控件。可编辑 ValuePattern 直接设置，Unicode 输入回退需要前台焦点。
- `press_key`：传 `window_id`、命名 `key`、可选 `ctrl`/`shift`/`alt`；支持导航键、F1–F12、字母、数字。完成和正常错误后释放修饰键。macOS 额外支持 `cmd`/`meta`，Linux X11 支持 `meta`（Super）。
- `move_mouse`、`mouse_click`：前台目标，整数屏幕 `x`/`y` 须在其边界内，命中测试拒绝遮盖窗口。按钮为 left/right/middle。

全部动作声明 ComputerUse 能力、RequiresApproval 安全级别，以及对 `desktop:global` 独占写入。定向动作还需一次、会话或持久应用访问，通用 shell/Process 不授权。保留独立文件／终端边界及动作审批。授权／撤销见[主机权限](host-permissions.zh-CN.md)。原生无障碍对象只在一次性 helper 内。Unix 节点 ID 组合层级位置和当前原生身份／角色／名称／边界，目标变化拒绝旧 ID。UI 变化后重新读，尤其缺稳定无障碍标识的 macOS provider。

<a id="privacy-and-limitations"></a>
## 隐私与限制

密码节点脱敏，绝不读取其值。截图前独立原始树扫描最多 20000 节点，包含用户可见读取上限之外的节点。密码框、不可访问 provider、不完整扫描或达到扫描上限均不提供图像。保护覆盖由应用元数据暴露的密码控件，自定义控件必须如实暴露元数据。

Windows PrintWindow、macOS 指定窗口 Quartz/ScreenCaptureKit、Linux X11 目标 drawable 捕获都不回退桌面复制。Linux 比对 X server 客户端 PID 与 AT-SPI D-Bus 连接 PID，捕获／坐标动作前拒绝重叠窗口。无法验证归属、非标准像素格式、Wayland 和不可用 provider 均不捕获。图像保持比例，不放大小窗口，限制长边。PNG 路径位于系统临时存储，主机传输返回内联图像，不开放主机文件系统。部分 GPU 渲染器、提权应用和最小化窗口无法捕获／操作。捕获失败返回 `screenshot_skipped`，UI 树仍可使用；不绕过系统保护／提权桌面权限。截图不自动删除，因为代理可能稍后读取。

<a id="modules-and-validation"></a>
## 模块与验证

CLI `computer_use.rs` 管理设置，runtime/builder.rs 控制注册。工具 `desktop.rs` 负责验证、权限及 helper 生命周期；`desktop_windows.rs` 使用安全的 uiautomation、enigo、win-screenshot、image API。`desktop_unix.rs` 保留 Rust 配置／应用授权，惰性运行内嵌 `desktop_unix/{worker,linux,macos}.py`，不引入 unsafe Rust 或任意 Python/PowerShell 求值。

当前原生验证覆盖 Windows fixture 与 Linux X11 GTK fixture，包括 Unicode 输入、控件、焦点、键鼠、图像限制、应用拒绝／撤销、变化 ID 和返回树范围外密码框。macOS 隐私／按键逻辑有隔离测试，真实 Apple 框架、TCC、窗口及捕获仍需 Mac 验收。受限 Wayland 路径未做合成器验收。这些结果不能证明缺失的原生 OS 沙盒已实现。
