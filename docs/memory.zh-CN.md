<a id="memory"></a>
# 记忆

[English](memory.md) | **简体中文**

AX 将原始会话历史、有效上下文快照和作用域事实保存在 SQLite。上下文压缩仅改变有效上下文，绝不将摘要晋升为全局／项目记忆，也不删除原始消息。磁盘布局和 schema 见[存储](storage.zh-CN.md)，可移植导出／导入见[备份](backup.zh-CN.md)。

<a id="scopes-and-writes"></a>
## 作用域与写入

事实有三个作用域：

- **Global**：属于当前安装的 AX 主目录（`<install-dir>/.ax`），任意项目可检索。
- **Project**：属于安装目录所拥有项目存储中的 UUID，与其他项目隔离，工作区删除后仍保留。
- **Session**：属于单个会话 ID，新会话不继承。

支持结构化声明，不增加模型调用：

```text
remember response.detail=brief
project: remember build.command=cargo test
global: remember response.language=English
```

首例仅属于当前会话。自然语言请求由主模型通过会话绑定的 `memory` 工具解释。工具可列出、设置或删除事实，指引模型将临时或含糊请求放在 Session 作用域，只有用户要求复用时才持久保存。变更需附当前用户请求逐字摘录，以证明来源；含义和授权仍由模型判断。

更改同一事实时，模型先列出现有事实并复用其键。更新／删除需提供精确旧值，过时写入失败，不覆盖新数据。自然语言陈述不再分配句子哈希键；旧哈希记录仍可读取，使用原键更新或删除。

所有事实写入 API 检查键值是否为空、超限或匹配已知凭据模式。检索也排除未通过验证的旧记录。这是保守过滤，不保证识别所有秘密格式。原始对话历史独立保留，不受此过滤脱敏。

<a id="project-identity"></a>
## 项目标识

项目根发现定位仓库或项目目录。UUID 归属独立于 `--data-dir`，元数据位于安装目录拥有的项目存储。旧工作区 ID 仍可读取，迁移时保留。已知安装标识还能从子目录识别普通项目根。删除工作区内容仍保留已存标识与历史。没有旧 ID 的新路径获得新标识，通过导出／导入转移数据。

旧路径归属记录以事务迁到 UUID。任何存储都允许匹配当前旧路径；默认项目本地数据库在移动后可接纳单个旧路径；共享自定义数据库绝不从不匹配路径推断归属。冲突时已有 UUID 值优先，原记录仍可导出，迁移标记防止删除事实复活；共享自定义库内旧无归属项目事实需显式迁移。

<a id="retrieval-and-context-budget"></a>
## 检索与上下文预算

检索分四步，均在本地，不用嵌入、向量库或额外模型调用：

1. 过滤归属，仅当前 Session、当前 Project UUID 和 Global 参与。其他项目／会话不召回。被取代或过期记录仍可经管理 API 访问，但此处排除。同键活跃项优先级保持 Session > Project > Global。
2. 用现有语言无关词法特征（NFKC、大小写折叠、单词和 Unicode n-gram）、精确键匹配、标签及路径，最多召回 32 个候选。只有显式固定的 Global 记录可绕过相关性下限，偏好键本身不等于固定。
3. 根据相关性加上有界的小幅使用量、置信度和类型相关时效奖励轻量重排。作用域和来源不计分。使用次数按对数增加，32 次饱和；置信度是 0–100 显式整数，旧默认 50。
4. 最多注入六条索引摘要，每条为最多 160 个 Unicode 字符的前缀。完整值由会话绑定 `memory` 工具通过 `action="read"`、`scope` 和 `key` 获取；`action="index"` 浏览摘要。现有 `list`、`set`、`delete` 和作用域存储 API 继续支持。

记忆类型定义自内容 `updated_at` 起按真实时间衰减：

| 类型 | 时效半衰期 |
|---|---|
| `preference` | 不衰减，需显式替换或设过期 |
| `fact`、`decision`、`reference` | 180 天 |
| `experience` | 30 天 |
| `task` | 3 天 |

`usage_count` 和 `last_used_at` 追踪实际注入的摘要及详情读取，使用不改变内容时间戳。`source` 记录来源，不代表重要性。新 `set` 可指定 `memory_type`、`confidence`、`tags`、`paths`、`superseded` 和 `expired`；旧客户端更新保留未提供元数据。结构化声明默认 `fact`，学习记忆用 `experience`。

SQLite `memory_index` 保存摘要和序列化词法特征，不存完整正文。写入预计算特征，导入及旧记录在首次检索对应归属时惰性建立。触发器在编辑时使索引失效，删除记录时删除索引，旧 SQL 路径也适用。建索引后日常检索只读摘要／特征，正文按需获取，保留现有有界词法缓存。

旧数据库以增量事务升级，保留键、值、作用域、归属、来源及时间戳。旧固定记录和 `preference.*` 键迁为 `preference`；其他默认 `fact`、零使用、置信度 50、无最后使用时间、活跃。旧序列化备份采用相同安全默认值；新备份导入保留全部元数据并惰性重建索引。

总大小含包装必须符合 `ContextBudget::memory_budget_tokens()`，最多估算 512 Token，小窗口进一步减少。过大候选跳过，使较小相关事实仍可放入。来源和时间戳在管理器及工具列表查看，不在每次注入上下文重复。最终请求选择仍强制整体上下文预算，见 [context.md](context.zh-CN.md)。

`[memory.retrieve]` 记录候选、匹配、注入、因预算丢弃数量及 Token／预算，不记录事实正文。每用户轮次开始检索，工具更新立即返回模型，注入记忆块于下一用户轮次刷新。

<a id="persistence-and-resume"></a>
## 持久化与恢复

CLI 在执行推进前为每条完整用户消息、助手响应和工具结果保存检查点。持久化失败停止轮次。尚未构成完整响应的流式文本不保存检查点。压缩快照在轮次末、原消息保存后写入，以最新持久化消息 ID 为锚。

恢复从快照水位之后按每页 128 条倒序查询，读到足够近期历史满足 Token 预算且有用户轮次边界后停止。持久代理状态为旧摘要单独恢复。最新轮次特别大时仍可能多页。初始 UI 对话也只载最新页，完整历史由 `/memory` 中独立操作查看。

崩溃前刚保存的助手工具调用可能没有结果。恢复追加明确中断结果标记，**不重放工具**，因为外部副作用可能已发生；指引模型先检查现状再重试。

<a id="user-controls"></a>
## 用户控制

- **`/memory`**：打开 Global、Project、Session 视图。选事实查看值预览、来源、更新年龄和纳入设置。编辑打开文本编辑器，Alt+Enter 保存，Esc 取消。删除只移除当前作用域事实。验证或并发更新导致编辑拒绝时会报告，不覆盖现值；关闭重开记录以加载新基线。
- **会话操作**：还提供原始历史、当前压缩摘要、手动压缩（`/compact`）及清空 Session 事实。
- **删除会话**：通过现有会话删除路径移除其 Session 事实、消息和摘要。

<a id="reference"></a>
## 参考

低频 [Evolution](evolution.zh-CN.md) 还可从重复 Experience 学习 Project 事实，复用验证及乐观写入，需要引用用户证据，以 `evolved` 标记来源，绝不覆盖用户事实。普通轮次不新增学习模型调用。

| 职责 | 代码 |
|---|---|
| 存储仓储、作用域、恢复 | `crates/memory/src/`（`store.rs`、`session.rs`、`message.rs`、`context.rs`、`events.rs`）、`crates/memory/src/scoped.rs` |
| 记忆工具与上下文注入 | `crates/cli/src/memory_tool.rs`、`crates/cli/src/memory_context.rs` |
| 事实视图及编辑器 | `crates/cli/src/tui/commands/memories.rs`、`crates/cli/src/tui/bottom_pane/memory_editor.rs` |
| 项目标识 | `crates/cli/src/project_identity.rs` |
| 会话恢复 | `crates/cli/src/session_restore.rs` |
