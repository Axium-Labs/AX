<a id="javascript-mods"></a>
# JavaScript Mod

[English](mods.md) | **简体中文**

Mod 是可执行会话扩展，受本地 Deepseek-harness 的 `register(on, options)` 桥接启发。AX 经已有作用域注册表发现 Mod；提示包含启用 Mod 时，CLI 组合根惰性启动 Node worker。目录查询、安装和开关操作不执行 JS。Core 只拥有可选 `RuntimeExtension` 边界，不依赖 Node。

<a id="package-and-management"></a>
## 包与管理

安装含 `mod.json` 及编译后 JavaScript 入口的目录：

```json
{
  "name": "notes",
  "description": "Add project context and a local command",
  "version": "1.0.0",
  "entry": "index.mjs",
  "enabled": true,
  "userConfig": { "note": "Run the project checks before finishing." }
}
```

入口必须是包内相对 `.mjs` 或 `.js` 文件。`.js` 使用 ES import 时需 `.mjs` 或 ESM `package.json`；TypeScript 先编译。`userConfig` 支持字符串、数字、布尔和字符串列表，清单值覆盖 `defineMod` 默认值。无效清单报错含包路径。安装拒绝符号链接和已有目标，经暂存目录复制，不覆盖其他包。

```bash
ax capabilities mods add notes --scope project --source /packages/notes
ax capabilities mods list --scope project
ax capabilities mods disable notes --scope project
ax capabilities mods enable notes --scope global
ax capabilities mods remove notes --scope project
```

全局包位于 `$AX_HOME/mods/<name>`，项目包位于 `<project>/.ax/mods/<name>`。`config.toml` 中 `[mods]` / `[mods.overrides]` 与 Skills/MCP/Agents 共用覆盖／屏蔽策略。Project 中移除或禁用继承全局 Mod 只在本项目屏蔽。`/mods` 和 `/settings` 提供 TUI 管理；Crew 插件页支持添加、搜索、详情、作用域、启用／禁用和移除。在已安装 `mod.json` 中编辑 `userConfig`。ACP 暴露 `_ax/mods` 及 `kind: "mods"` 的 `_ax/scopedCapabilities`；目录行除作用域／状态外还含 source、version 和 userConfig。

<a id="hooks-commands-and-tools"></a>
## Hook、命令与工具

`index.mjs` 可导出 `register`、包含 `register` 的默认规范，或使用 `@ax/mods` 的 `defineMod`。Deepseek-harness 包导入名也映射到此小型 `defineMod` shim；其他依赖必须先在包内安装。

```js
import { defineMod } from '@ax/mods'
export default defineMod({
  name: 'notes',
  userConfig: { note: 'Check the project.' },
  register(on, options) {
    on('session.start', async ($, event, next) => {
      await $.command.register({ name: 'note', description: 'Show project note' })
      await $.tool.register({
        name: 'note', description: 'Read the configured project note',
        inputSchema: { type: 'object', properties: {} },
      })
      return next(event)
    })
    on('prompt.submit', async ($, event, next) =>
      next({ ...event, context: [...event.context, options.note] }))
    on('command.run', { command: 'note' }, async () => ({ text: options.note }))
    on('tool.call', { tool: 'mcp__notes__note' }, async () => ({ result: options.note }))
  },
})
```

CLI/TUI/Crew 聊天可执行 `/mod:note`。命令不调用模型，持久化结果并完成其 Goal。`session.start` 注册的工具以 `mcp__<mod>__<tool>` 进入真实模型 schema，经普通权限边界执行。不支持或重复名称会被拒绝。在 session start、schema 冻结前注册命令／工具。

支持事件：`session.start`、`session.end`、`turn.start`、`prompt.submit`、`tool.call`、`command.run`、`turn.complete`。匹配器支持精确值、值列表及 RegExp。Hook 必须返回结果或 `next(event)`；多次 `next` 共用同一 promise，不重复执行底层工具。`.catch(handler)` 可恢复 hook 失败。`prompt.submit` 可修改模型输入及增加系统上下文，原用户历史保留。`tool.call` 可观察、替换输出或返回 `{deny: "reason"}`。经 `next` 修改已批准参数会被拒绝，应新发 `$.tool.call` 进行独立权限检查。`turn.complete` 仅为观测事件，返回值不能重写已流式发送答案。

<a id="state-and-host-operations"></a>
## 状态与主机操作

`$.state.get/set/delete` 保存会话临时值，`get` 返回 `{value}`。缓存 worker 在同一活跃会话的 ACP 运行时重建间保留状态。`$.store.get/set/delete` 在 `$AX_HOME/mod-store/<name>.json` 持久化各 Mod JSON，限 4 MiB，原子替换文件。并发会话最后写入者获胜，不是事务。移除包保留 store。`$.session` 暴露会话 ID、cwd、模型、轮次数和有效消息；usage 报告已知上下文窗口，不虚构 Token 数或账户速率限制。

`$.tool.call` 调用基础主机工具，重新检查权限／profile。`$.fs.write` 使用文件系统工具，`$.http.fetch` 使用 web fetch。工作区读取（`read/exists/list/stat`）拒绝规范化后项目外路径，文件读取限 4 MiB。`$.clock` 支持 now/sleep/after/every，会话销毁清除定时器。`$.env` 读写 worker 环境。UI log/toast/status 发往 stderr，不污染 ACP stdout。`ui.open` 返回 `isPlaced:false`，不渲染 UI 面板。

这是 AX Mod API，不是完整 DSH/Claude SDK 兼容层：`ui.render`、`ui.ask`、托管层（`next.to`）、编程式 `prompt.submit`、进程 API 及其他事件／命名空间明确失败。主机工具查找捕获初始注册表，之后每轮记忆／技能工具不能经 `$.tool.call` 调用。父 Mod 不复制到隔离子内核。直接 import 仍可使用普通 Node API，helper 边界不是 JS 沙盒。

<a id="execution-requirements-and-lifecycle"></a>
## 执行要求与生命周期

PATH 需有 Node.js 20.6+，或以 `AX_MOD_NODE` 指定可执行文件。无启用 Mod 就无 Node 进程。Mod 是可信可执行代码；AX 工作区沙盒模式非 off，或本地运行时在 SSH 上下文执行时，拒绝加载。远程 Mod 应在远程 AX 主机安装运行。

各 hook 预算 10 秒，catch handler 1 秒，整个桥接事件 15 秒。错误／超时明确报告，失败 worker 终止。取消事件绝不重放，已发生副作用无法回滚。在保留本地内核中 worker 失败后需新会话；ACP 下一轮重建会话时替换失败 worker。清单／配置／启用／入口变更在下一提示重载，导入依赖变化需要新会话。重载重置临时状态和工具注册，持久 store 不变。新建／删除会话及进程退出时销毁 worker；突然终止不保证 `session.end`。
