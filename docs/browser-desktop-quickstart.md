# 浏览器和电脑操作快速入门

先查看 [权限与沙箱](host-permissions.md) 和 [Computer Use](computer-use.md)。

## 电脑操作

原生 Windows 中运行 `ax computer-use --enabled true`，或在 AXCrew 设置的
电脑操作页开启。首次访问目标应用需要授权：仅此次、本会话或始终允许。
具体操作仍遵循审批。应用权限绑定真实进程的可执行文件路径，不是窗口标题。
使用 list_windows 发现目标，再用 read_window 获取控件 ID；更改后重新读取。
截图只覆盖目标窗口，密码和不完整隐私扫描会阻止截图。

## 浏览器

安装 Node，再运行（将路径替换为实际 AX_HOME）：

```sh
npm install --prefix "<AX_HOME>/browser" playwright
npx --prefix "<AX_HOME>/browser" playwright install chromium
```

AX 创建独立浏览器，不共享日常浏览器的登录配置。网站权限可在 AXCrew 浏览器
设置中管理，或使用 `ax host-permissions`。每个协议、域名、端口分别授权。

```json
{"action":"open","url":"https://example.com","session":"main"}
{"action":"snapshot","session":"main"}
{"action":"fill","target":"input[name=search]","text":"AX","session":"main"}
{"action":"click","target":"button[type=submit]","session":"main"}
{"action":"screenshot","session":"main"}
{"action":"close","session":"main"}
```

使用 CSS 选择器，不使用外部 Playwright CLI 的 ref。跨站资源、重定向、
WebSocket、弹出窗口、上传和下载暂被阻止；访问其他网站要明确请求 goto 并授权。
部分依赖 CDN 或第三方登录的网站不能完整工作。密码页面不返回快照或截图。
截图通过已有图片通道返回，不需要额外允许读取主机文件。

权限分离不会更改文件/终端沙箱。Windows/macOS 原生工作区沙箱后端仍未实现，
明确请求 workspace/strict 仍会拒绝；非 Windows 的电脑控制尚未实现。
