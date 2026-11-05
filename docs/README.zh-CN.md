<a id="ax-documentation"></a>
# AX 文档

[English](README.md) | **简体中文**

本目录介绍 AX 当前行为、架构与开发。特定版本变更记录于[发布记录](releases/)和[旧版发布说明](release-notes/)，长期设计决策记录于 [ADR](adr/README.zh-CN.md)。

<a id="documentation-map"></a>
## 文档导航

跨平台构建、shell 设置和能力限制见 [platforms.md](platforms.zh-CN.md)。

原生桌面自动化及其 AXCrew 设置契约见 [computer-use.md](computer-use.zh-CN.md)。独立应用／网站访问、浏览器会话和沙盒／审批边界见 [host-permissions.md](host-permissions.zh-CN.md)。

| 文档 | 主题 |
|---|---|
| [agent-runtime.md](agent-runtime.zh-CN.md) | 运行时契约：请求／上下文／能力边界、轮次流程、触发条件 |
| [coding-harness.md](coding-harness.zh-CN.md) | 执行策略、有界错误驱动交接审查、具体工作、工作区规范及持久产物 |
| [long-project.md](long-project.zh-CN.md) | 可选持久工作上下文、作用域模板、安全 Session 交接及恢复 |
| [goal.md](goal.zh-CN.md) | 可选目标执行、验证、生命周期、预算与恢复 |
| [agent-loop.md](agent-loop.zh-CN.md) | 续接状态、流式完成、错误驱动交接反馈与可选 Goal |
| [architecture.md](architecture.zh-CN.md) | 系统架构：crate 职责、Agent Loop、冷启动路径 |
| [distributed-collaboration.md](distributed-collaboration.zh-CN.md) | 可选持久跨 Host Task、Event、Artifact、Workflow State 及 AX worker |
| [acp-crew.md](acp-crew.zh-CN.md) | ACP 适配器和 AX Crew 设备桥接 |
| [memory.md](memory.zh-CN.md) | 记忆：作用域、项目标识、检索和持久化 |
| [context.md](context.zh-CN.md) | 上下文预算、压缩和会话恢复 |
| [storage.md](storage.zh-CN.md) | 存储：SQLite schema、JSONL 事件流、数据目录和迁移 |
| [backup.md](backup.zh-CN.md) | 带版本 axpack 导出／导入、数据边界及项目重映射 |
| [tools.md](tools.zh-CN.md) | 工具、权限与安全 |
| [security.md](security.zh-CN.md) | 运行时工作区沙盒、操作系统约束及逃逸测试 |
| [browser-desktop-quickstart.md](browser-desktop-quickstart.zh-CN.md) | 浏览器传输与原生桌面入门 |
| [web-search-quickstart.md](web-search-quickstart.zh-CN.md) | 搜索使用与配置 |
| [web-search-implementation-guide.md](web-search-implementation-guide.zh-CN.md) | 搜索适配器维护 |
| [web-search-architecture.md](web-search-architecture.zh-CN.md) | 网页搜索：两层原生优先路由、提供商能力及配置 |
| [mcp.md](mcp.zh-CN.md) | MCP 集成：配置、传输及惰性连接 |
| [skills.md](skills.zh-CN.md) | 技能：包格式、路由、启用／禁用 |
| [capabilities.md](capabilities.zh-CN.md) | 共用 Global/Project 注册表及 Skill/MCP/Agent/Mod 管理 |
| [mods.md](mods.zh-CN.md) | 可执行 JavaScript Mod、加载、hook、命令、工具及兼容性 |
| [evolution.md](evolution.zh-CN.md) | 低频 Experience 学习和进化 Skill 生命周期 |
| [providers.md](providers.zh-CN.md) | 模型、提供商与认证 |
| [contributing/README.md](contributing/README.zh-CN.md) | 贡献流程、issue、PR 和审查策略 |
| [development.md](development.zh-CN.md) | AX 构建、测试、扩展及发布 |
| [releases/0.3.17.md](releases/0.3.17.zh-CN.md) | 当前发布版本 |
| [adr/README.md](adr/README.zh-CN.md) | 架构决策记录索引 |

旧发布说明按版本保存在 `release-notes/`。本地[网页搜索调度基准](web-latency.zh-CN.md)供维护者参考，不属于产品功能指南。

<a id="how-to-read"></a>
## 阅读方式

- **新用户**：从仓库 [README](../README.zh-CN.md) 开始，了解安装、首次会话和提供商。
- **Codex／AI 代理**：从 [AGENTS.md](../AGENTS.md) 开始，按任务导航到对应文档。
- **贡献者**：从 [CONTRIBUTING.md](../CONTRIBUTING.md) 了解 issue、实现和审查流程，技术标准遵循 [development.md](development.zh-CN.md)。
- **深入阅读**：[storage.md](storage.zh-CN.md)、网页搜索审计及 [adr/](adr/README.zh-CN.md) 包含实现和设计参考。

<a id="documentation-conventions"></a>
## 文档约定

运行时性能：[runtime-benchmarks.md](runtime-benchmarks.zh-CN.md) 介绍 AX-Bench Phase 1 及离线测量边界。

Agent 能力与高级执行：[agent-benchmarks.md](agent-benchmarks.zh-CN.md) 介绍生产 CLI 任务套件、Crew 适配器及未实现边界。

- 每文件一个主题，通过引用避免重复。
- 原文语言与仓库保持一致，使用英文。
- 有长期影响的决策记为 ADR，见 [adr/README.md](adr/README.zh-CN.md)。
- 添加或重命名文档时更新本导航。
- 测试结果、机器特定路径及任务进度放 `logs/`，不放当前功能文档。功能文档只记录可复现、与用户相关的限制。
- 网页搜索实现及验证限制见 [web-search-architecture.md](web-search-architecture.zh-CN.md) 和 [web-search-audit.md](web-search-audit.zh-CN.md)。
