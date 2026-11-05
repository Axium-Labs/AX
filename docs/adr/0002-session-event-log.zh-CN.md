<a id="0002-sqlite-index--jsonl-session-event-log"></a>
# 0002. SQLite 索引与 JSONL 会话事件日志

[English](0002-session-event-log.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

AX 保存的原始会话历史需要经受崩溃、压缩及长时间会话。将完整消息正文内联存入 SQLite 会使数据库膨胀，增加备份、审计和截断难度；全部仅用普通 JSONL 保存又会使元数据查询和分页变慢。压缩需要标记“已摘要到哪里”的水位，且不能删除任何原始内容。

<a id="decision"></a>
## 决策

- 使用**混合布局**：SQLite 保存会话／消息元数据、索引和事实；完整消息保存在各会话的追加式 JSONL 事件流（`sessions/<uuid>.jsonl`）。
- **JSONL 是权威来源，SQLite 是可重建索引。**消息行通过 `event_offset`/`event_length` 指向 JSONL；事件写入后清除内联内容。
- **先写 JSONL，再写 SQLite**（位于 `IMMEDIATE` 事务中），避免丢失事件，索引始终可重建。
- 每次读写前运行 `sync_session_locked`，检测缺失或过时索引、从 JSONL 重建，并迁移旧内联行，使崩溃恢复自动且幂等。
- 压缩写入各会话摘要及 `through_message_id` 水位，绝不删除原始事件。

<a id="consequences"></a>
## 影响

- 相比纯 SQLite，磁盘开销约为 1.1–1.2 倍，换取完整审计链和崩溃安全写入。
- 新旧版本不能同时使用同一数据目录；旧版本忽略 JSONL，会产生不一致。
- 手工编辑 JSONL 会破坏精确偏移索引，不受支持；备份需同时复制 SQLite 和 JSONL 文件。
- 从旧纯 SQLite 布局自动透明迁移，见 [storage.md](../storage.zh-CN.md)。
