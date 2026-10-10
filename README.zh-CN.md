# AX

用 Rust 构建的快速、轻量终端 AI Agent。

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](Cargo.toml)
[![Rust 1.92+](https://img.shields.io/badge/rust-1.92+-orange)](https://www.rust-lang.org)

[English](README.md) | **简体中文**

AX 是一个小巧的原生终端 Agent：启动快、流式响应，会话、记忆、凭证与模型配置都留在你的机器上——无需注册任何服务，没有远程遥测。

AX 是 Agent；[AX Crew](https://github.com/Axium-Labs/AXCrew) 是与它一同构建的控制平面——连接、控制并编排运行在笔记本、服务器、GPU 服务器与云虚拟机上的 AX Agent。参见 [ACP 与 Crew 集成](docs/acp-crew.md)。

- **Rust 原生** — 核心终端 Agent 为单一二进制，不需要 Node 或 Docker；可选的浏览器操作需要 Node 和 Playwright。
- **独立界面权限** — 应用、网站访问与具体操作审批分别管理，不改变文件和终端的沙箱。Windows 电脑操作默认关闭；平台支持和浏览器限制见 [主机访问权限](docs/host-permissions.md)。
- **启动快** — 供应商、技能、MCP 服务器与记忆按需加载。
- **本机资源** — `/system` 按需采集真实 CPU、内存以及受支持 NVIDIA 显卡的使用率与显存；读取失败明确显示不可用。
- **标准技能与聚焦工具** — Agent Skills `SKILL.md` 包（惰性指令 + 可选资源）、只读的 web 搜索/抓取（并发批量查询与页面抓取并去重结果）、通过可配置 MCP 服务器接入的 LSP，以及受支持视觉模型上的原生图片输入。
- **二进制小巧** — 紧凑的聚焦 crate 工作区，不是框架。
- **简单工作流** — 输入、得到回答、切换模型、继续。
- **本地优先** — 会话、记忆与设置存放在 AX 可执行文件旁的 `.ax` 中。

暂无公开 benchmark；这里的"快"指的是启动路径在设计中尽量少做事。

默认情况下，AX 将持久化数据存放在 `<install-dir>/.ax` 下，项目级数据在 `projects/<project-key>/` 下。打开其他工作目录不会把数据搬过去；删除工作区不会删除其历史。首次启动时，旧版 `~/.ax` 与已知项目 `.ax` 存储会被复制（不删除原文件）。`AX_HOME` 与 `--data-dir` 仍是显式覆盖项。

便携备份：`ax export backup.axpack` 导出会话与记忆；用 `--memory` 或 `--sessions` 选择类型。先运行 `ax import backup.axpack --dry-run` 检查冲突，再运行 `ax import backup.axpack` 合并。凭证与缓存不包含在内。详见 [备份细节](docs/backup.md)。

## 快速开始

### 安装并运行 AX

Mac 或 Linux：

```bash
curl -fsSL https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.sh | sh
```

Windows：

```powershell
powershell -ExecutionPolicy Bypass -c "iex ((iwr 'https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1' -UseBasicParsing).Content)"
```

然后直接运行：

```text
ax
```

第一个会话：

```text
/login      # 用供应商登录（Codex OAuth 或 API Key）
/model      # 选择模型
hello       # 直接开始输入
```

就这些。会话自动保存，所选模型下次启动时会被记住。

## 安装

安装器从 GitHub Releases 拉取最新版本，用该版本的 `SHA256SUMS` 校验压缩包，安装单个 `ax` / `ax.exe` 二进制：

- **macOS / Linux** — 默认安装到 `~/.local/bin`，若该目录不在 PATH 中会打印提示。
- **Windows** — 默认安装到 `%LOCALAPPDATA%\Programs\AX\bin`，并加入用户 PATH（仅当尚未存在时）。

两个安装器都接受两个环境变量：

| 变量 | 作用 |
|---|---|
| `AX_VERSION` | 安装指定 release tag 而非最新版（默认：最新） |
| `AX_INSTALL_DIR` | 覆盖安装目录 |

要把当前正在运行的二进制更新到最新 GitHub Release，运行 `ax --update`。AX 检查 release 版本、用 `SHA256SUMS` 校验下载并替换该二进制；同时从同一压缩包刷新内置负载，把缺失的技能包安装到 `<install-dir>/.ax/skills`，且仅在尚无配置时写入 `<install-dir>/.ax/mcp.toml`（已有技能与配置保持不变）。每次 `ax --update` 都会刷新内置负载（即使二进制已是最新），因此新增技能包无需升版本即可生效。在 Windows 上，校验过的更新会在命令退出后计划替换；重启 AX 后再使用新版本。该命令不会改动你的 `<install-dir>/.ax` 用户数据或任何项目的 `.ax` 目录。若你用 `AX_INSTALL_DIR` 安装了独立副本，请运行该副本的 `ax --update` 更新。仅推送 `main` 不算发布；发布工作流针对 `v*` tag 运行。

```bash
curl -fsSL https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.sh | sh
AX_INSTALL_DIR="$HOME/bin" curl -fsSL https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.sh | sh
```

```powershell
powershell -ExecutionPolicy Bypass -c "iex ((iwr 'https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1' -UseBasicParsing).Content)"
```

### 卸载

移除 AX 只需删除二进制（Windows 上还要删除安装器加入的 PATH 项）。AX 的数据单独存放在 `<install-dir>/.ax`，想彻底清空也可以一并删除。

**macOS / Linux**

```bash
rm -f ~/.local/bin/ax        # 或 AX_INSTALL_DIR 指向的位置
rm -rf ~/.local/bin/.ax                 # 配置、会话、记忆、模型目录（可选）
```

**Windows**

```powershell
Remove-Item -Force "$env:LOCALAPPDATA\Programs\AX\bin\ax.exe"
# 移除安装器加入的 PATH 项（可选但更整洁）
$p = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path', ($p -split ';' | Where-Object { $_ -notlike '*Programs\AX\bin*' }) -join ';', 'User')
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Programs\AX"   # 一并删除存储的会话与记忆（可选）
```

## 开发

### 从源码构建

需要 Rust 1.92+。

```bash
git clone https://github.com/Axium-Labs/AX.git
cd AX
cargo build --release
# 二进制位于 target/release/ax.exe（Windows）或 target/release/ax（Unix）
```

## 供应商与模型

AX 从你机器上已有的东西发现供应商——`<install-dir>/.ax/auth.json` 中的凭证与 `DEEPSEEK_API_KEY` 等标准环境变量。启动时不发生任何网络调用。

| 供应商 | 凭证 | 用法 |
|---|---|---|
| OpenAI Codex | OAuth 登录 | `/login` → Codex OAuth |
| WorkBuddy International | 浏览器 OAuth + token 轮询 | `ax auth login workbuddy --region intl`，或 `/login` → WorkBuddy International |
| WorkBuddy China | 浏览器 OAuth + token 轮询 | `ax auth login workbuddy --region cn`，或 `/login` → WorkBuddy China |
| DeepSeek | API Key | `/login` → API Key，或 `DEEPSEEK_API_KEY` |
| OpenAI | API Key | `/login` → API Key，或 `OPENAI_API_KEY` |
| OpenAI 兼容供应商（Groq、Mistral、OpenRouter 等） | 供应商 API Key | `/login` → 供应商，或其 API Key 环境变量 |

模型动态发现并写入 `<install-dir>/.ax/models/`（内置目录 + 各供应商刷新缓存）。随时用 `/model` 切换；上次选择——供应商、模型与推理强度——持久化到 `<install-dir>/.ax/config.json`，下次启动恢复。

从命令行显式指定供应商：

```bash
ax --provider deepseek
ax --provider openai-codex --model <model-id>
ax run "explain this repo" --provider deepseek
```

`--provider` 接收目录中的供应商 id，而不仅是 `deepseek` / `openai` / `codex` / `compatible` 别名。供应商出现多次时使用其名称——小米的普通条目与 CN/SGP/AMS token 套餐区域共享模型 id 但端点不同：

```bash
ax run "explain this repo" --provider xiaomi-token-plan-cn --model mimo-v2.5
```

## 用法

启动 UI（默认）：

```bash
ax
```

运行单个任务后退出（非交互）：

```bash
ax run "summarize the TODOs in ./src"
```

以有界并发运行独立任务：

```bash
ax agents "review pr #12" "write changelog" --concurrency 2
```

要把 AX 现有运行时暴露给 ACP 客户端，运行 `ax acp`。要把本机连接到 AX Crew 后端，先运行 `ax crew pair <code> --gateway <https-url>` 一次，再运行 `ax crew connect <https-url>`。参见 [ACP 与 Crew 集成](docs/acp-crew.md)。

## 设计

**快。轻。简单。**

AX 刻意不一次性加载所有东西。技能、MCP 服务器、供应商与记忆只在任务真正需要时激活——让 Agent 强大的东西从不妨碍启动或普通对话。这是有意的取舍：能力按需加载，而非启动时加载。

## 文档

内部架构——Agent 循环、上下文管理、多 Agent 调度、SQLite schema、模块布局——记录在 [docs/README.md](docs/README.md)。从 [docs/architecture.md](docs/architecture.md) 开始。

## 许可证

MIT 或 Apache-2.0。

### 导入技能与 MCP

```powershell
ax skill import C:/downloads/my-skill           # 项目 skills/
ax skill import C:/downloads/my-skill --global  # <install-dir>/.ax/skills
ax mcp import C:/downloads/mcp.json             # 安装目录 .ax/projects/<project-key>/mcp.toml
ax mcp import C:/downloads/mcp.toml --global     # <install-dir>/.ax/mcp.toml
```

导入会校验内容、保留现有名称，不运行脚本或 MCP 服务器。MCP JSON 接受常见的 `mcpServers` 格式。

## 本地编码基准

见 `benchmark/README.md`：固定的三任务 AX vs Codex 运行器、隔离要求与实测结果。包含的这次运行在编码前被网关 403 阻断，不构成性能排名。

## 可选的分布式协作

AX 0.3.7 / AXCrew 0.3.3 新增基于 Durable Task、Event、Artifact 与 Workflow State 的协作层。AXCrew 按 AX 能力与 Host 资源统一分配任务；AX 保持现有推理、Memory 和 Subagent 能力，无须持续存活的 Coordinator Agent。在 AXCrew 侧边栏「分布式协作」注册实例、下载配置，再在目标机器运行 `ax crew worker worker.json`。支持一台 Host 多个 AX、一个 AX 多个 Execution，普通单机与原有设备控制仍可独立使用。见 [配置与恢复边界](docs/distributed-collaboration.md)。
