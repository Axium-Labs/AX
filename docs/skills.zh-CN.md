<a id="skills"></a>
# 技能

[English](skills.md) | **简体中文**

AX 使用 [Agent Skills 规范](https://agentskills.io/specification)。技能提供任务指令，不在启动上下文加入正文。Skill、MCP、Agent 作用域管理共用[注册表](capabilities.zh-CN.md)。

<a id="package-format"></a>
## 包格式

```text
skills/<skill-name>/
├── SKILL.md            # required YAML frontmatter and Markdown body
├── scripts/            # optional executable resources
├── references/         # optional documentation
└── assets/             # optional static resources
```

必填 `name` 为 1–64 个小写 ASCII 字母、数字或单个连字符，不得以连字符开头／结尾、含连续两个连字符，且必须与父目录同名。必填 `description` 为 1–1024 字符，应说明用途及使用时机。

AX 保留可选标准字段 `license`、`compatibility`（1–500 字符）、`metadata`（字符串映射）和 `allowed-tools`（空格分隔字符串）。有效未知 frontmatter 扩展字段保留。AX 专属信息放在 `metadata`，如 `ax.required-tools: "filesystem search shell"`，使缺少工具时不能激活技能；仅用于真实依赖。`allowed-tools` 是给代理的声明，绝不授予权限。每个工具调用仍经过注册表及 AX 权限检查。

示例：

```markdown
---
name: code-review
description: Review changes for regressions. Use when reviewing a diff or commit.
license: Apache-2.0
compatibility: Requires git.
metadata:
  author: axium-labs
  version: "1.0"
allowed-tools: filesystem search shell
---
# Review

Inspect the full diff and report actionable regressions.
```

<a id="bundled-code-review-scope"></a>
## 内置代码审查范围

内置 `code-review` 遵循用户要求的范围。Diff／commit／PR 审查默认聚焦新引入、可采取行动的回归；明确要求扩大审查时可包含已有问题，分别标明。全项目审计检查指定模块及交互，不局限近期 diff；风格／约定审查采用用户要求的标准。始终需证据、具体影响及文件／行引用。未要求修复时只读。范围指引不增加运行时过滤或工具权限。

<a id="discovery"></a>
## 发现

首次路由或 `/skills` 时 AX 建索引，只读取各 `SKILL.md` YAML frontmatter。共用注册表先载全局 `$AX_HOME/skills`，再载项目 `<project>/.ax/skills`，仍可读旧 `<project>/skills` / `--skills-dir`。项目同名覆盖全局，规范项目包先于旧包。每个根内路径排序，同目录标准 `SKILL.md` 优先于旧文件。无效及重复包逐项在 stderr 和 `/skills` 报告，不影响有效技能。

发布归档包含仓库内置包，安装器放入 `$AX_HOME/skills`，通常为 `<install-dir>/.ax/skills`，对所有项目生效。已存在包不覆盖，使本地编辑在升级后保留；`ax --update` 也以相同方式从新发布刷新。技能本质是普通目录，手工安装可复制至该位置或 `<project>/.ax/skills`。

<a id="routing"></a>
## 路由

技能激活由模型决定并受策略约束。AX 一次索引紧凑元数据并预计算 Unicode 词法特征。`route_candidates()` 以最强相似信号排列符合条件的名称／描述，没有激活权重或置信阈值。每个合格技能，包括弱词法匹配，都可能进入有界紧凑目录。主模型判断相关性，显式调用 `invoke_skill(name)`，不增加规划器调用。只有该工具调用才读取验证正文，并返回技能根目录；资源保持惰性。

启用状态、Global/Project 覆盖／屏蔽、包路径、依赖及 `allow_implicit_invocation: false` 是确定性准入过滤器。仅手动技能必须由当前用户输入明确点名才合格，选择不能扩大合格集合。`allowed-tools` 仅元数据，不授予运行时权限。旧 `auto_route_candidates()` API 现在仅返回显式名称候选，CLI 不用它加载正文。`route()` 返回排名提示。

目录、已加载指令、记忆、摘要、历史及工具结果竞争同一弹性上下文池。大包不预留固定窗口份额。Evolution 使用遥测来自实际成功调用，而非路由预测。

<a id="legacy-compatibility"></a>
## 旧格式兼容

同目录没有 `SKILL.md` 时，已有 `skill.toml` 加 `instructions.md` 包继续加载，保留旧工具依赖。路由忽略旧 `trigger_keywords`。新包应使用 `SKILL.md`，内置 `skill-creator` 与 `skill-installer` 创建安装此格式。

<a id="enable-and-disable"></a>
## 启用与禁用

`/skills` 列出索引包并可切换。选择通过与 MCP/Agents 相同策略存入所选作用域 `config.toml`，旧 `disabled-skills.json` 作为回退。项目禁用继承全局包保存本地屏蔽。禁用从有效上下文移除该技能标记指令，但保留已存历史。重新启用允许后续匹配轮次路由。

<a id="bundled-skills"></a>
## 内置技能

仓库五个内置标准包为 `coding`、`code-review`、`skill-creator`、`skill-installer` 和 `web-research`；后者为 `web` 工具提供批量、基于来源的研究策略。

<a id="self-evolving-skills"></a>
## 自我进化技能

CLI 在项目和全局技能之后，还发现 `<data-dir>/evolution/<project-id>/live` 中的 trial/active 包。候选和归档不参与路由。Evolution 复用标准 skill-creator 指令与 Skill 验证器，归属元数据独立，自动变更仅限进化包。调度、生命周期和控制见 [evolution.md](evolution.zh-CN.md)。

<a id="reference"></a>
## 参考

| 职责 | 代码 |
|---|---|
| 解析、验证、发现、路由 | `crates/skill/src/lib.rs` |
| Unicode 特征及相似度，与记忆检索共用 | `crates/lexical/src/lib.rs` |
| CLI 路由与上下文注入 | `crates/cli/src/repl/state.rs`（`prepare_skill_context`、`ranked_skill_catalog_context`） |
| 启用与禁用 | `crates/cli/src/skill_settings.rs` |
| `/skills` UI | `crates/cli/src/tui/commands/catalogs.rs` |

<a id="importing-a-local-skill"></a>
## 导入本地 Skill

`ax skill import C:/downloads/my-skill` 将验证后包安装到当前项目 `.ax/skills/`，加 `--global` 使用 `$AX_HOME/skills`。`--skills-dir` 仍为发现来源。AX 复用标准包验证器，复制资源，拒绝符号链接和冲突名称，导入不执行包脚本。Crew 设置 → AX 能力暴露相同命令。
