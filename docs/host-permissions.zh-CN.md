<a id="host-access-sandbox-and-action-approvals"></a>
# 主机访问、沙盒与动作审批

[English](host-permissions.md) | **简体中文**

AX 分离三个决定：

- 文件／终端隔离：现有 `SandboxManager` 和 `WorkspaceWorker` 工具执行配置工作区边界。主机访问不改变模式，不在边界外重试被拒命令。
- 主机访问：`AuthorizedHost` 工具解析真实应用可执行文件或精确 HTTP(S) origin，要求独立访问。Process、shell、Network 和 `--allow-dangerous` 授权不覆盖未知应用／网站。
- 动作：每次工具调用仍受既有能力规则、任务权限上限和审批策略约束。已允许的 Computer Use 应用也授权该应用普通动作，避免反复通用提示。显式任务 Ask/Deny 和能力 Deny 仍优先；浏览器动作审批独立。

调度器先解析访问，再做普通动作审批，取得类型化授权，经 `execute_output_authorized` 传递。执行前工具重新解析目标并检查撤销。Mod 内置调用共用此门槛，工作区包装器转发授权契约。模型输入字段不能创建访问权。

<a id="grants-and-settings"></a>
## 授权与设置

未知目标默认 `ask`，可选一次、本会话、始终或拒绝。一次只覆盖当前调用，不保存。会话授权属于单一控制器权限作用域和 AX 主目录，其他 ACP／聊天会话不能复用；会话重置、恢复／新会话边界或进程退出即到期。持久允许／拒绝存于 `AX_HOME/host-permissions.json`，独立于 `config.json`。显式拒绝和浏览器关闭优先于先前批准。删除保存规则后恢复询问，并清除对应会话授权。

文件含 `browser_enabled`，默认 true，及 `apps`、`sites` 映射，值为 `allow`、`ask`、`deny`，另有不透明 `versions` 元数据。各目标 epoch 使另一 UI／CLI 进程撤销／移除后，旧会话授权失效，不重置无关目标。关闭再开启 Browser Use 也使临时授权失效。应用身份是规范可执行路径，Windows 大小写规范化，从真实窗口进程解析。网站身份含 scheme、host、port，不是通配域名或页面路径；HTTP／HTTPS、子域及非默认端口各不相同。

```sh
ax host-permissions
ax host-permissions --surface computer --target "C:/Program Files/Example/app.exe" --decision ask
ax host-permissions --surface browser --target https://example.com --decision allow
ax host-permissions --surface browser --target https://example.com --decision deny
ax host-permissions --surface browser --target https://example.com --remove
ax host-permissions --browser-enabled false
```

前置 CLI 无需工作区／模型／会话，读取不创建文件，无效编辑在写入前失败。写入使用进程间锁与原子替换，保留无关应用／网站条目、AX 配置和凭据。权限损坏时拒绝访问。AXCrew 经类型化 `ax_host_permissions` 桥接修改相同规则，只显示已确认值；ACP 权限对话框支持四种选择。

<a id="computer-use"></a>
## Computer Use

原生桌面 worker 是主机传输，授权独立于受限文件／终端 worker。配置缺失时 Computer Use 默认开启；显式保存的禁用仍有效，只能经用户设置路径改变。默认开启不授予应用权限。最小窗口发现安全，定向操作需应用访问。`list_windows` 仅暴露最小应用／句柄元数据，隐藏拒绝应用，未持久允许时不显示标题。定向读取、截图和动作需应用授权。不新增任意全桌面截图或 shell 命令。图像用已有多模态 `ToolOutput::Image`，临时存储不授予任意主机文件访问。见 [computer-use.md](computer-use.zh-CN.md)。

<a id="browser-use"></a>
## Browser Use

<a id="axcrew-embedded-transport"></a>
### AXCrew 嵌入式传输

本地 AXCrew 桌面以 `AX_CREW_BROWSER_URL` 和启动秘密启动 AX 时，`browser` 使用桌面认证回环桥接和真实侧栏 WebView，CLI 默认不变。每次操作前同一 HostPermissionStore 和任务策略解析网站访问。Windows 嵌入式传输要求覆盖所有来源、无法保护即拒绝的 WebView2 请求过滤、重定向拦截和 WebSocket 阻断；运行时／平台不可用则报错，不另启浏览器。桥接复用用户嵌入式 cookie／登录 profile；标签按 AX Session 隔离，profile 不隔离。首次接管先安装保护再重载原页。支持快照文本、CSS 目标、click/fill/type 和 Enter/Tab；截图和其他键明确失败。受控视图跨 Crew 对话切换保留，经标签控件或 `browser close` 关闭。桌面桥接环境凭据从 shell、沙盒、MCP、Mod 子进程清除，只可信本地 AX/ACP 保留，SSH 及配对远程 Host 不继承，不写入设置。

<a id="standalone-playwright-transport"></a>
### 独立 Playwright 传输

不在显式 AXCrew 桌面传输内时，`browser.rs` 在文件／终端 worker 之外，经 Node 和 Playwright 惰性启动内嵌 `browser_worker.mjs`。创建 AX 拥有的非持久 Chromium context，启用 Chromium 沙盒，不用个人 profile。仅同一控制器权限作用域跨轮次共享 context。关闭最后会话即关闭 Chromium；丢弃作用域关闭驱动输入管道，允许正常清理。Worker 截止时间关闭所属会话，Rust 交互另有超时。

此独立传输需显式安装外部运行时，AX 不会从浏览器工具调用安装软件：

```sh
npm install --prefix "<AX_HOME>/browser" playwright
npx --prefix "<AX_HOME>/browser" playwright install chromium
```

需有 Node。可信主机环境覆盖为 `AX_BROWSER_NODE`、`AX_BROWSER_PLAYWRIGHT`、`AX_BROWSER_CHANNEL`（如 `msedge`）和 `AX_BROWSER_HEADLESS=true`，不是模型工具参数。

动作：`open`、`goto`、`snapshot`、`screenshot`、`click`、`fill`、`type`、`press`、`reload`、`go_back`、`go_forward`、`list_sessions`、`close`。`target` 是 CSS 选择器，不是 Playwright CLI 元素引用。会话名限 1–64 个 ASCII 字母／数字／连字符／下划线。导航须用不含内嵌凭据的 HTTP(S)。Browser Use 有独立能力；读取快照／截图属安全动作，但仍需网站访问。域名 profile 拒绝及任务网络上限在传输层仍有效。

当前策略刻意限于活跃已授权 origin。跨来源资源、导航、iframe 请求和各次重定向在发送前阻止；访问另一网站需显式 `goto` 并授权来源。不可用项包括 service worker、WebSocket、弹窗、下载、上传及模型提供任意 JavaScript，可能使依赖 CDN、第三方登录或 WebSocket 的网站失败。手动重定向抓取保留请求页面 URL，不暴露最终同来源重定向 URL，可能影响相对链接。页面内容不可信；这些是当前功能限制，不是完整兼容承诺。

无障碍快照传输上限 32000 字符，标题／URL 也有界。含密码页面不提供快照和截图；密码／自定义控件保护依赖真实无障碍／HTML 元数据。截图返回内联 PNG，不导入个人 cookie 或登录 profile。请求错误去掉调用日志，避免返回 Cookie/Authorization 头。取消交互关闭管道，不会留下队列响应污染后续调用。

<a id="platform-and-trust-limits"></a>
## 平台与信任限制

权限分离不会补足缺失操作系统后端。目前真实原生 workspace/strict 隔离仅 Linux；原生 Windows/macOS 显式受限仍拒绝，平台默认 off。原生 Computer Use 有 Windows UIA、Linux AT-SPI、macOS Accessibility 适配器。Unix 需文档依赖，macOS 还需系统隐私授权。Wayland 目前只允许无障碍动作。WSL 可操作 Linux GUI，无 Windows 桌面 IPC 桥接；SSH 不暴露本机 UI。独立浏览器需受支持 Node/Playwright/浏览器安装，此处在 Windows Edge 实测过。本地 AXCrew 嵌入式 Agent 控制则需文档所述 Windows WebView2 请求守卫，无需外装 Node/Playwright，不支持平台明确失败。

这些设置管理 AX 可信工具入口，不管理同一系统用户下已运行的任意程序。不受限 shell 或控制通用终端／浏览器应用可能执行更广动作。受支持时保持现有 OS 沙盒，应用／网站检查是独立服务边界，不能替代系统隔离。本地原生 AX 扩展仍属可信计算基。

架构决策见 [ADR 0024](adr/0024-independent-host-access.zh-CN.md)。
