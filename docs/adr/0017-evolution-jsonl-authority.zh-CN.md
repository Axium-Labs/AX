<a id="0017-authoritative-experience-jsonl-and-evolution-control-checkpoints"></a>
# 0017. 权威 Experience JSONL 与 Evolution 控制检查点

[English](0017-evolution-jsonl-authority.md) | **简体中文**

状态：已接受

<a id="context"></a>
## 背景

Evolution 在有界 JSON 账本及追加式 JSONL 中重复保存完整观测。近期窗口去重会再次追加旧 ID；JSONL 已追加但控制检查点失败时，账本无法恢复。

<a id="decision"></a>
## 决策

完整 Experience 记录只属于 `experiences.jsonl`。版本 2 的 `ledger.json` 包含 Skill 生命周期元数据、epoch、调度计数器，以及分别用于已观测使用量和成功分析的字节游标。近期证据是有界运行时缓冲。worker 保留 Engine；初始化时一次流式建立内存 ID／偏移索引，此后只从追加字节扩展。分析选择连续待处理前缀，不能提交未包含记录。

旧格式迁移先为原边界保存检查点，再追加缺失 ID，最后发布仅含控制信息的账本。冲突以 JSONL 为准。现有动作、Skill 格式、Memory 和项目作用域契约不变。不新增数据库或 crate：sessions/experiences/decisions 使用 JSONL，Memory／可查询索引用 SQLite，Evolution 控制用 JSON，Skills 用 `SKILL.md`。

<a id="consequences"></a>
## 影响

分析或持久化失败保留待处理游标。重启从权威流重建近期证据及去重信息。临时 ID／偏移索引随记录数增长，初始化扫描一次历史。跨存储动作在崩溃时仍为至少一次；保守迁移可能重分析此前已消费但副本缺失的记录。撕裂或损坏原始记录会拒绝继续，不做破坏性修复。旧 AX writer 不能共享迁移后的存储。旧账本副本派生的纠正注释不替代原始 JSONL；Skill 纠正计数保留。
