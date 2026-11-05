<a id="portable-backup-and-restore"></a>
# 可移植备份与恢复

[English](backup.md) | **简体中文**

AX 通过存储仓储导出用户数据。命令：

    ax export backup.axpack
    ax export backup.axpack --memory
    ax export backup.axpack --sessions
    ax import backup.axpack --dry-run
    ax import backup.axpack

导出不带选项时同时选择记忆与会话，同时传两个选择选项也选择两者。导出目标必须是新文件，绝不覆盖已有归档。导入以 JSON 输出计划或完成数量和冲突列表。Dry-run 验证整个归档，以只读方式打开已有数据库，不创建项目标识或数据库。

<a id="format"></a>
## 格式

axpack 是 ZIP 归档，精确包含以下条目：

| 条目 | 内容 |
|---|---|
| manifest.json | 格式 axpack、版本 1、AX 版本、创建时间、包含的数据类型、各数据条目的 SHA-256 和记录数 |
| metadata.json | 来源项目的可移植 UUID |
| memories.jsonl | 带 global、project、session 作用域的事实 |
| legacy_memories.jsonl | 带来源存储（global 或 project）的旧扁平记忆记录 |
| sessions.jsonl | 会话 ID、标题及时间戳 |
| messages.jsonl | 会话顺序的完整原始消息，以可移植序号代替 SQLite ID |
| summaries.jsonl | 摘要内容、消息序号水位和可选有效上下文快照 |

包中没有 SQLite 数据库、WAL、JSONL 事件偏移或绝对项目路径。写入前导入验证每条目的 SHA-256、大小、记录数、引用、ID 和格式版本。支持版本 1，未知版本明确失败，为未来格式迁移留出空间。当前读取器限制每个解压条目 128 MB，总计 512 MB。

<a id="included-data-and-boundaries"></a>
## 包含数据与边界

默认导出包含会话、原始消息、摘要及有效快照，带 global/project/session 作用域的事实，以及旧扁平记忆。读取当前项目数据库及 AX 主目录全局记忆数据库。不符合现有记忆验证器的无效事实或疑似凭据事实会省略。

绝不打包 auth.json、API 密钥、OAuth 令牌、MCP 配置或凭据、提供商配置、模型缓存、临时文件、语言服务器进程状态或整个 AX 数据目录。对话消息是用户编写数据，本身可能含敏感文字或工具输出。分享前检查目标归档，AX 不尝试脱敏任意对话历史。

<a id="merge-and-project-identity"></a>
## 合并与项目标识

导入保留已有数据。会话 ID 匹配时跳过整个传入会话，包括其消息、摘要和会话事实。已有作用域记忆键或旧记忆键保留，跳过传入值。仅记忆包若不在同包包含对应会话，无法恢复会话事实；这些事实作为孤立会话记忆报告并跳过。

包中项目事实携带来源 UUID，不携带旧路径。导入将其映射到当前项目安装目录所属 project.json 中的 UUID，不覆盖当前标识。旧项目数据库有需要时，导出先应用现有路径归属迁移。仅实际合并时创建当前项目标识，dry-run 不创建。

导入先暂存新会话 JSONL 事件，在数据库事务中插入 SQLite 索引和项目／全局事实，然后在提交前发布事件。报告失败时回滚数据库并移除暂存文件。SQLite WAL 跨附加项目及全局数据库事务不能保证两个文件的崩溃原子性。在狭窄事件发布窗口崩溃，可能留下未索引的孤立 JSONL；重试时 AX 不会静默覆盖该文件。正常会话操作仍以现有 JSONL 事件为权威。

<a id="code"></a>
## 代码

CLI 路由到 memory::backup::{ExportService, ImportService}。服务经现有 MemoryStore 读取会话及消息，导入时重建本地 SQLite 消息 ID 和 JSONL 偏移，不注册 Agent 工具。
