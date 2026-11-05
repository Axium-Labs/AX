<a id="storage"></a>
# 存储

[English](storage.md) | **简体中文**

AX 使用 SQLite、JSONL 和 JSON 混合架构：SQLite 快速查询元数据和索引，JSONL 为各会话提供完整不可变事件流，JSON 保存轻量配置。原始历史不丢失，SQLite 是可重建索引。

<a id="data-directory-layout"></a>
## 数据目录布局

持久状态放在运行可执行文件旁 `.ax`，显式配置时用 `AX_HOME`。工作目录放用户项目文件及可选用户技能，不放默认运行时状态。`--data-dir` 仍可显式覆盖选定项目存储。

旧数据复制保护比较规范路径分量，包含 Windows 盘符和 `\\?\` 规范路径。创建暂存目录前跳过相同目标或来源内部目标。因此 ACP 以用户主目录作工作区时，不会递归复制自身 AX 主目录、项目存储或迁移暂存目录；原数据保留。

```text
<install-dir>/
├── ax(.exe)
└── .ax/
    ├── auth.json
    ├── config.json
    ├── memory.sqlite3              # Global facts
    ├── session-projects.json       # cross-project session picker
    ├── models/
    ├── skills/
    ├── mcp.toml
    └── projects/<project-key>/
        ├── project.json            # Project UUID
        ├── memory.sqlite3          # Project/Session facts and metadata
        ├── memory.sqlite3-wal
        ├── memory.sqlite3-shm
        ├── sessions/<uuid>.jsonl    # authoritative raw history
        ├── mcp.toml                # optional project override
        └── evolution/              # learned skills and Experiences
```

项目键为规范项目路径 SHA-256，Windows 先规范大小写。项目存储独立，事实仍以项目 UUID 归属。接受旧工作区标识并复制到安装存储。删工作区文件不影响标识和历史。没有旧 ID 的新路径是新项目；有意迁移到另一位置／安装时用可移植导出／导入。

启动时复制旧用户主目录和注册项目存储，当前项目旧 `.ax` 在首次使用复制。保留已有目标文件，不删除来源，完成后标记以免反复扫描。SQLite writer 锁保护数据库快照和 JSONL 复制，backup API 包含已提交 WAL 写入，先发布 JSONL 再数据库。安装目录不可写则报错，不回退 cwd。显式 `AX_HOME` 跳过旧用户目录迁移，显式 `--data-dir` 保留。

<a id="sqlite-schema"></a>
## SQLite schema

数据库（`memory.sqlite3`，WAL，`PRAGMA user_version = 7`）包含：

| 表 | 用途 | 关键列 |
|---|---|---|
| `sessions` | 会话元数据 | `id`（PK）、`title`、`created_at`、`updated_at` |
| `messages` | 消息索引 | `id`（PK）、`session_id`（FK，级联）、`role`、`kind`、`content`、`metadata`、`created_at`、`event_offset`、`event_length` |
| `agent_states` | 跨压缩保留的系统指令／技能 | `message_id`（PK，FK）、`session_id`、`content`、`metadata`、`created_at` |
| `session_summaries` | 每会话一行：累计摘要及压缩水位 | `session_id`（PK）、`content`、`compressed_message_count`、`updated_at`、`through_message_id` |
| `long_term_memory` | 旧扁平记忆 | `id`（PK）、`key`（唯一）、`value`、`category`、`created_at`、`updated_at` |
| `scoped_memories` | Global / Project / Session 事实 | `scope`、`owner`、`key`（复合 PK）、`value`、`source`、`updated_at`、`always_include`、`memory_type`、`usage_count`、`confidence`、`last_used_at`、`superseded`、`expired`、`tags`、`paths` |
| `memory_index` | 惰性词法索引及短摘要 | `scope`、`owner`、`key`（复合 PK）、`summary`、`features`、`valid` |
| `memory_migrations` | 作用域事实迁移标记 | `category`（PK）、`migrated_at` |
| `project_memory_migrations` | 路径归属 → UUID 迁移标记 | `owner`（PK）、`project_id` |

说明：

- 事件写入 JSONL 后，`messages` 的消息**正文**清空为 `content=''`、`metadata='null'`，JSONL 是权威。`event_offset` / `event_length` 指向会话文件。
- 索引包括 `sessions(updated_at DESC)`、`messages(session_id, id
  DESC)`、`long_term_memory(category, updated_at DESC)`、`agent_states(session_id, message_id)`。
- schema 按列／表存在性进行增量迁移，`PRAGMA user_version` 报最新版本。Memory v7 在事务升级，保留旧行，新元数据用安全默认值。
- 记忆索引触发器在插入或正文／标签编辑时使缓存特征失效，删除时移除索引。旧／导入记忆仅为查询归属惰性计算特征，摘要展示前完整验证。普通写入预计算特征。
- `usage_count` 限于 `u32`，置信度整数 `0..=100`。读取独立更新使用计数，不改内容时间戳。

<a id="jsonl-event-streams"></a>
## JSONL 事件流

每会话一个 `sessions/<session-uuid>.jsonl`，每行是完整 `StoredMessage` JSON：

```jsonl
{"id":1,"session_id":"...","role":"user","kind":"message","content":"user input","metadata":null,"created_at":1234567890}
{"id":2,"session_id":"...","role":"assistant","kind":"message","content":"AI response","metadata":{"tokens":150},"created_at":1234567891}
```

JSONL **仅追加**，崩溃安全且完整，包含已压缩消息，易备份，可独立于 SQLite 审计。

<a id="write-path"></a>
## 写入路径

`append_message` 在 `IMMEDIATE` 事务内：

1. `sync_session_locked` 恢复检查，确保索引最新。
2. 在 `messages` 插占位索引行获取新 ID。
3. 构建完整 `StoredMessage`。
4. 追加 JSONL 并 `fsync`，原子、O(1)。
5. 更新 `event_offset`、`event_length`。
6. 类型为 `AgentState` 时另存 `agent_states`，使其跨压缩保留。
7. 更新会话 `updated_at`。
8. 提交。

**有意先写 JSONL，再写 SQLite。**事件不丢失，索引可重建。

<a id="read-path"></a>
## 读取路径

`load_messages` 先恢复检查，再以 `LIMIT`/`OFFSET` 分页查询索引并解码：

- 有 `event_offset`/`event_length` 的索引行：seek 到 JSONL，精确读 `length` 字节，验证 `event.id` / `event.session_id` 与索引匹配。
- 迁移前旧行：从 SQLite 内联解码。

<a id="crash-recovery"></a>
## 崩溃恢复

每次读写前，`sync_session_locked` 同步 JSONL 与索引：

1. 统计未索引消息和索引结束位置。
2. 比较 JSONL 大小。
3. 全同步则走快路径返回。
4. JSONL 小于索引末尾，报“missing JSONL events”。
5. 否则扫描 JSONL，补索引并恢复 `agent_states`。
6. 旧内联消息读取正文、追加 JSONL、更新指针、清正文。

崩溃场景：

| 时机 | JSONL | SQLite | 恢复结果 |
|---|---|---|---|
| 追加 JSONL 前 | 未变 | 未变 | 消息丢失，符合预期，事务未提交 |
| `fsync` 后、索引前 | 完整事件 | 缺索引 | `sync_session_locked` 重建 |
| 索引后、提交前 | 完整事件 | 回滚 | 下次写入幂等重建 |
| 提交后 | 完整事件 | 完整索引 | 正常 |

<a id="migration"></a>
## 迁移

<a id="old-inline-sqlite-messages--jsonl"></a>
### 旧 SQLite 内联消息 → JSONL

每会话首次访问时自动迁移：读取 `event_offset IS NULL` 的旧消息，追加 JSONL，更新指针，再清 SQLite 正文，不丢数据。

<a id="legacy-project-identity"></a>
### 旧项目标识

先读安装 `project.json`，再读旧工作区 `project.json` 或 `project-id`，原子发布 UUID 到安装拥有的项目存储，保留旧文件，不在工作区写新元数据。

<a id="legacy-configtoml--configjson"></a>
### 旧 `config.toml` → `config.json`

`AxConfig::load_from_home` 读 `config.json`，缺失时解析旧 `config.toml`，另存 `config.json` 后继续。

<a id="consistency--concurrency"></a>
## 一致性与并发

- **单进程多线程**：`IMMEDIATE` 事务和 `busy_timeout(5s)` 串行写入。
- **多进程**：JSONL 仅追加，天然安全；SQLite WAL 和 `IMMEDIATE` 自动排队。事务锁避免两进程生成不同 `id` 的风险。
- **读取验证**：JSONL 的 `event.id`、`event.session_id` 不匹配 SQLite 索引时，`decode_indexed` 报错。

<a id="performance"></a>
## 性能

- **写**：JSONL O(1) 追加加 SQLite O(log N) 索引，含 `fsync` 约 1–2 ms／消息。
- **读**：分页 O(log N + K)，JSONL seek 随机读 O(1)，只载所需消息。
- **开销**：SQLite 索引每消息约 100–200 字节，加 JSONL 实际消息，约纯 SQLite 的 1.1–1.2 倍，换取审计和崩溃安全。
- **压缩**：仅载最新 N 条，1000 条、500 KB 会话载最新 50 条，JSONL 读取约少 95%。

<a id="backup--troubleshooting"></a>
## 备份与排障

- 可移植备份用 `ax export backup.axpack`，见[备份](backup.zh-CN.md)。
- 单安装底层灾难恢复需数据库与 JSONL 一起保存。
- 新旧 AX 不应共用数据目录，旧版忽略 JSONL，可能不一致。
- 不手改 JSONL，索引用精确偏移。
- JSONL 损坏／删除时从备份恢复，或删会话重新开始；下次访问从 SQLite 重试迁移。
- 确认迁移后用 `VACUUM;` 回收 SQLite 空间。

<a id="reference"></a>
## 参考

| 职责 | 代码 |
|---|---|
| 仓储与同步 | `crates/memory/src/store.rs`、`crates/memory/src/events.rs` |
| schema 与迁移 | `crates/memory/src/schema.rs`、`crates/memory/src/migrations.rs` |
| 作用域记忆 | `crates/memory/src/scoped.rs` |
| 项目标识迁移 | `crates/cli/src/project_identity.rs` |
| 配置迁移 | `crates/cli/src/config.rs` |

<a id="durable-task-queues"></a>
## 持久任务队列

同一队列的可选 `goal` 对象保存目标验收、生命周期、累计资源使用、上次验证及恢复预留，不另设 ID 或存储头。普通队列不含它，见 [goal.md](goal.zh-CN.md)。

队列检查点为带 `[ax-task-queue]` 前缀的普通 `AgentState`，含 `goal_id`、可选 `parent_goal_id`、队列生命周期、原目标、有序任务、失败原因、恢复尝试及缓存最终响应。Pending/running 条目决定剩余工作，经既有 JSONL 优先路径和状态索引持久化，不增 schema。

`latest_agent_state(session, prefix)` 独立于历史分页和压缩取最新状态。加载恢复元数据，不隐式继续；新用户 Goal 取代活跃／暂停队列，将其归档到 `[ax-task-queue-archive]`，再写新状态头。显式恢复需保存的 Goal ID，旧快照恢复时获取稳定 ID。Blocked/cancelled/completed 跨重连保持终态，预算耗尽挂起 Goal。ACP 用户取消等待已中止 prompt 任务后才保存取消，避免晚检查点复活队列。仅断开连接保留可恢复工作。

模型上下文排除完整状态和归档消息；更换提供商保留队列元数据。中断工具调用保留已有恢复占位，不重放副作用。

<a id="child-session-persistence"></a>
### 子会话持久化

ACP 为普通委派及队列子任务在父会话保存紧凑 `[ax-subagent]` `AgentState` 索引，保留执行 ID、可选直接父 ID、任务、生命周期／摘要和私有 `ChildRun` 定位，经既有路径写入，不迁 schema。这些 UI 恢复索引不计模型上下文和 Token。子文字／工具留在独立权威 JSONL；ACP/Crew 去私有定位，从所属子存储恢复活动。验证及回放界限见 [ACP 委派活动](acp-crew.zh-CN.md#delegated-agent-activity)。

队列任务可含 `child` 描述（`goal_id`、`session_id`、`cwd`、`state_dir`、`memory_scope`、执行预算），执行前在控制器队列保存身份。各子拥有 `state/child.sqlite3` 及独立 JSONL；原始模型／工具消息仅追加，不膨胀控制器快照。Global/project/session 记忆操作均绑定子存储及唯一归属。

`[ax-child-outcome]` 终态回执先于控制器推进保存。重连运行描述加载相同工作区和会话；有回执则直接返回记录，无需再调用模型／工具，即使工作区已删。恰在回执写前中断，可由持久最终助手消息恢复回执。Pending 子任务用新身份，已完成／失败绝不重启。复制工作区排除控制器存储及会话流。

`state/workspace.json` 保存所属 cwd、Git 仓库、生命周期及最后检查点时间，准备的子生命周期内持有 `state/workspace.lock`，中断释放并把未完成子标 `interrupted`。先写终态回执，再写清单终态及清理。GC 重试终态清理，过期无租约空闲工作区，保留状态目录。无 `state_dir` 的旧描述先整体迁旧 `.ax` 和 JSONL，再启清理。持久记忆及 shell AX_HOME 绑定 `state`，不在可丢弃 cwd 内。历史按时间载入，恢复仅追加新生成原消息。

ACP `session/load` 也回放原始队列检查点及归档引用的子工具，含取消／中断子任务。各子仅一次，沿用实时 `child-session:call-id`。参数、完成结果和未完成调用跨取消／重连可见，工作区清理后仍保留。子提示及内部助手文字仍只在子对话中。

<a id="evolution-storage-boundary"></a>
## Evolution 存储边界

Session、Experience、decision 是权威追加式 JSONL；Memory 及可查询索引用 SQLite；Evolution 控制用带版本 JSON 账本；Skill 仍标准 `SKILL.md`。不增加数据库或 crate 依赖。

各项目 Evolution 根中，`experiences.jsonl` 保存完整观测，`ledger.json` 保存生命周期、调度／聚合计数及观测遥测／成功分析的独立字节游标。近期证据仅在有界内存缓冲。ID／偏移索引在 worker 初始化时从 JSONL 重建一次，此后增量扩展。发布版本 2 前按 ID 协调旧账本副本。迁移、失败恢复和旧 writer 限制见 [Evolution](evolution.zh-CN.md)。

<a id="concurrent-acpssh-initialization"></a>
## 并发 ACP/SSH 初始化

schema 迁移读取元数据前预留 immediate SQLite 写事务，避免多个本地 AX 同开项目库时 WAL 读快照升级失败。作用域索引迁移共用写预留。会话项目注册表更新加文件锁、写唯一暂存文件后原子重命名，读取者看不到截断 JSON，并发写保留各注册项目。

<a id="optional-goal-session-handoffs"></a>
## 可选 Goal Session 交接

Long Project 无需 schema 迁移。现有 agent-state 保存 `[ax-session-handoff]` 初始化意图、`[ax-session-predecessor]`、`[ax-session-successor]` 激活提交及可选 `[ax-recent-tool-results]` 最新轮次原始缓存，经相同 JSONL 优先 MemoryStore 索引及有效上下文水位。队列保持权威，没有对应前驱提交的后继不能执行。重定向保留历史 Session 行／流，使同 Goal ID 跨多个 Session。Markdown 投影和过滤修订归档位于显式 Goal 目录，不替代 SQLite 或 Checkpoint。交接事务和分布式限制见 [Long Project](long-project.zh-CN.md)。
