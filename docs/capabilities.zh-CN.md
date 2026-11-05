<a id="scoped-capabilities"></a>
# 作用域能力

[English](capabilities.md) | **简体中文**

Skill、MCP 服务、命名 Agent 和 Mod 共用 `scoped::ScopedRegistry<T>`、配置解析器和管理操作。CLI 只提供元数据适配，Crew 使用 AX ACP 接口，不自行合并配置。

<a id="locations"></a>
## 位置

```text
$AX_HOME/
  skills/<name>/SKILL.md
  agents/<name>.toml
  agents/<name>.md
  mods/<name>/mod.json
  mcp.toml
  config.toml
<project>/.ax/
  skills/<name>/SKILL.md
  agents/<name>.toml
  agents/<name>.md
  mods/<name>/mod.json
  mcp.toml
  config.toml
  project.json
```

AX 沿用安装目录拥有的 AX 主目录及 `AX_HOME` 覆盖。`AX_HOME=~/.ax` 可使用惯常用户主目录布局。能力配置独立于会话库 `--data-dir`。旧 `<project>/skills` 和显式 `--skills-dir` 仍可读，优先级在规范 `.ax/skills` 之后。`--mcp-config` 替代项目 MCP 来源，全局服务仍合并。规范项目 MCP 配置缺失时，已有安装仍可读取旧 `<data-dir>/mcp.toml`。

现有项目 UUID 标识项目。项目变更时，AX 按既有标识格式将同一 UUID 写入 `.ax/project.json`，移动项目时保留标识和本地配置。作用域策略及 Agent 清单不保存绝对工作区路径。

<a id="resolution"></a>
## 解析

始终先加载全局定义，再加载项目定义，应用项目覆盖、`disabled_global`，构建有效注册表。同名／ID 整体替换定义。禁用的项目定义仍遮蔽全局定义，不回退全局；只有启用项进入有效执行注册表。

各类型在 `config.toml` 使用相同策略格式：

```toml
[skills]
disabled_global = ["reviewer"]
[skills.overrides]
rust-dev = false

[mcp]
disabled_global = ["browser"]
[mcp.overrides]
issues = true

[agents]
disabled_global = ["reviewer"]

[mods]
disabled_global = ["notes"]
[mods.overrides]
metrics = false
```

可选顶层 `disabled_global = ["name"]` 在四类中都屏蔽该名称；名称重叠时优先类型专属屏蔽。启用会移除相应屏蔽。项目启用／禁用继承项不改写全局配置，项目定义不受全局名称屏蔽影响。`overrides` 将显式布尔值与定义分开持久化，保留无关执行／提供商配置。旧 `disabled-skills.json` 可回退读取，新显式策略优先。

<a id="management"></a>
## 管理

`/skills`、`/mcp`、`/agents` 和 `/mods` 显示名称、作用域和状态，包括 `[global]`、`[project]`、`enabled`、`disabled`、`disabled here`。Space 在选定配置作用域切换。`/settings` 提供**全局配置**和**当前项目配置**，再进入各类型。项目视图含继承全局项；全局视图即使项目有遮蔽也显示全局定义。

四个斜杠命令语法相同：

```text
/skills list
/skills list global
/mcp disable browser project
/agents enable reviewer global
/agents add reviewer project C:\templates\reviewer.toml
/skills remove rust-dev project
```

公共 CLI 适合脚本和 Crew：

```bash
ax capabilities skills list --scope project
ax capabilities mcp disable browser --scope project
ax capabilities agents add reviewer --scope global --source /templates/reviewer.toml
ax capabilities skills remove rust-dev --scope project
```

`add` 要求请求名称与来源定义一致，不覆盖目标已有定义。Skill 来源为包目录；MCP 来源为含 `[servers.<name>]` 的 TOML；Agent 来源为带相对指令文件的 TOML 清单。现有 Skill/MCP `import` 也安装到这些规范目录。Project 中移除继承全局项会创建本地屏蔽，移除项目定义则显露存在的全局定义。

ACP 提供 `_ax/skills`、`_ax/mcp`、`_ax/agents`、`_ax/mods` 及公共 `_ax/scopedCapabilities`。公共方法接受 `kind`、`scope`、`action`、`name` 和可选 `source`。变更需显式作用域；不指定作用域的列表返回含禁用元数据的有效视图。行含 `name`、`scope`、`status`、`enabled`、`description`、`source` 及适配器元数据。响应暴露既有 `project_id` UUID。Crew 设置使用这些行，每次变更均调用 AX。

Mod 来源为含 `mod.json` 及相对 JavaScript 入口的目录。目录查询只读元数据，启用 Mod 在会话拥有的 Node host 内惰性执行。包、hook、命令、持久状态、Node 要求及 API 限制见 [Mods](mods.zh-CN.md)。

<a id="named-agents-and-lazy-loading"></a>
## 命名 Agent 与惰性加载

```toml
# agents/reviewer.toml
name = "reviewer"
description = "Review code for regressions"
enabled = true
instructions = "reviewer.md"
tools = ["filesystem", "find_files", "search"]
```

可选 `tools` 列表收窄父工具，不能授予权限。模型用 `agent = "reviewer"`、`task` 和可选显式 `context` 调用 `subagent`，仅此时读取命名 Agent 指令。禁用名称被拒绝，且不进入工具 schema。未命名委派始终可用。

全局执行默认值仍在既有 `AxConfig` JSON。项目 `.ax/config.toml` 中 `[subagent]` 部分覆盖默认，例如 `max_concurrent = 2` 仅限该项目。深度预算为零时模型轮次不初始化命名 Agent 注册表。AXCrew 设置 → 插件 → Agents 经 `ax settings --scope global|project` 读写限制。深度 0 禁用，1 允许直接子代理，更大值允许后代。`--reset` 恢复全局默认或移除项目覆盖。后代保留父级启用的命名 Agent 目录，调用时才加载指令，不能扩大父工具或权限范围。见[委派限制](tools.zh-CN.md#subagents)。

Skill frontmatter 和 MCP/Agent 定义仅为元数据。禁用 Skill 不加载正文、不进入路由／目录上下文；禁用 MCP 不进入运行时管理器或启动传输。列表／启用／禁用不加载 Agent 指令。注册表首次使用后缓存，管理／进化变更时失效。管理能力无需提供商调用或连接，原始会话历史保留。
