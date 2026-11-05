<a id="browser-and-computer-use-quickstart"></a>
# 浏览器与 Computer Use 快速开始

[English](browser-desktop-quickstart.md) | **简体中文**

<a id="computer-use"></a>
## Computer Use

Computer Use 默认开启，除非显式禁用。运行 `ax computer-use` 查看当前后端及配置，`ax computer-use --enabled true` 可重新启用。Windows 使用 UIA，Linux 使用 AT-SPI，macOS 使用 Accessibility。按 [Computer Use](computer-use.zh-CN.md#platform-dependencies-and-os-permissions) 安装平台依赖并授予系统权限。

先用 `list_windows` 发现窗口，再用 `read_window` 读取，然后使用控件 ID。应用访问独立于文件／终端限制。已保存的应用允许涵盖普通动作，显式任务 Ask/Deny 与能力 Deny 仍有效。参见[主机权限](host-permissions.zh-CN.md)。

<a id="browser-use"></a>
## Browser Use

浏览器传输取决于启动上下文：

- 本地 AXCrew 桌面执行通过认证桥接使用侧栏 WebView。代理控制目前需要 Windows 及受支持的 WebView2 请求守卫，复用嵌入式浏览器 profile；不提供截图，按键仅支持 Enter/Tab。
- 独立 AX 使用 Node 和 Playwright，由 AX 管理非持久 Chromium context。按 [Browser Use](host-permissions.zh-CN.md#browser-use) 中的命令安装运行时。
- 配对设备或 SSH 执行不继承本机桌面浏览器桥接。

目标使用 CSS 选择器，例如：

```jsonl
{"action":"open","url":"https://example.com","session":"main"}
{"action":"snapshot","session":"main"}
{"action":"close","session":"main"}
```

每个 HTTP(S) origin 均需授权。跨来源资源及重定向、WebSocket、弹窗和下载受限，第三方登录或依赖大量 CDN 的页面可能无法工作。操作、profile／隐私限制及各传输支持见权威[浏览器契约](host-permissions.zh-CN.md#browser-use)。浏览器访问不改变文件／终端沙盒。
