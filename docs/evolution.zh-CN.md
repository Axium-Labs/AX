<a id="self-evolving-skills"></a>
# 自我进化技能

[English](evolution.md) | **简体中文**

AX 观察普通会话、学习可复用个人流程，不为每轮增加模型请求。`evolution` crate 扩展 CLI 组合边界，不依赖 runtime-core、工具、终端 UI 或权限策略。只可写自身私有 Skill 区域，以及通过现有 Memory 仓储写事实，绝不执行生成代码。

```text
Session → Experience → low-frequency analysis → candidate → trial → active
                                                   ↑         ↓       ↓
                                                   └── refine/merge ─┘
                                                          ↓
                                                      archived → deleted
```

<a id="observation-and-scheduling"></a>
## 观测与调度

CLI 包装现有 `AgentEvent` 回调，记录任务文字、工具名、路由 Skill 名及观察到的 Skill 文件读取、有序工具步骤、结果、失败、可能重试、项目和会话标识。工具乱序完成时以调用 ID 关联。不收集流式文字或私有推理。任务及详情有界，每 Experience 最多 64 步和 64 错误。重试表示观测失败后重复相同工具／详情，不推断外部副作用被重放。`success` 仅表示运行时完成轮次，不证明用户接受。含错误轮次保留为分析器解释的使用／错误遥测。取消丢弃记录器并记录中断失败。

普通请求交给有界非阻塞 worker 队列，文件 I/O、证据筛查及分析在进程内线程执行，分析经既有 Tokio runtime 使用配置提供商。不安装 daemon 或后台服务。队列 64 槽，过载诊断后可能丢学习记录，但权威会话完整。Evolution 失败不使用户轮次失败。

累积批次或会话结束且持久冷却已到期时可分析。`/new`、切换会话／项目通知会话边界。`ax run` 和空闲 TUI 退出会排空 worker，合格最终分析可能耗至超时。ACP 每提示重建组合状态，临时状态销毁仅刷新观测，不错误结束会话，因此使用累计批次触发。突然终止可能丢排队观测，已持久证据下次可用。进程退出中断的当前 TUI 轮次同其他待工作一样尽力处理。

<a id="detection-and-classification"></a>
## 检测与分类

一次有界、无工具模型请求读取近期 Experience 及所属 Skill，复用内置 `skills/skill-creator/SKILL.md`。将持久偏好／事实分为 **Memory**，重复流程分为 **Skill**，一次性／弱证据分为 **Ignore**。Intent 初始为任务文本，语义意图和纠正归因在分析时决定，不额外逐轮请求。REFINE 纠正须引用真实用户文字，并关联有界缓冲中的相应 Experience。JSONL 原观测不可变，纠正计数和 Skill 证据 ID 保留在生命周期元数据。

分析器提出 JSON 动作，不能选路径或调用工具。AX 用已有 Skill crate 的 `create_skill_directory` 和 `validate_skill_directory` 创建验证标准包，无另种格式或自定义 frontmatter。分析输入缩减到声明上下文容量，预留输出及 creator／分析指令，旧观测留磁盘。

<a id="actions-and-lifecycle"></a>
## 动作与生命周期

模型提出 **CREATE / MERGE / REFINE / IGNORE / RETIRE** 及 **MEMORY** 事实；**PROMOTE** 显式请求 candidate→trial 或 trial→active。运行时维护不因综合分数或年龄自动晋升、归档或删除。

确定性门槛仍包括至少 `minimum_evidence` 条观测、来自 `independent_sessions` 个不同非空会话，默认 2 和 2，以及有效项目来源、秘密过滤、包验证、归属／摘要保护、增长限制和使用遥测。Trial→active 需试用基线后真实成功使用。分析器 `confidence` 只是经验证遥测，不是生命周期阈值。失败观测可支持纠正经验，成功／失败语义由模型解释。

REFINE 保留终身遥测，修改活跃指令后重回 trial。MERGE 需不同所属来源且输出更小，但词法相似度不能否决语义合并。RETIRE 先归档，后续显式提案才可删除，且只删原单文件包。人工编辑及新增资源保护用户内容，绝不自动覆盖用户 Skill/Memory。

Candidate 位于发现之外。晋升 trial 需后续分析 epoch 和至少一个不在创建／细化证据中的新 ID；有效提案进入绑定内容摘要的用户审核，不自动晋升。空闲 TUI 展示指令，`y` 接受为 trial，`n` 归档，Esc 留待后续 TUI；上下键滚动。包变化使旧审核失效。ACP、`ax run` 保留 candidate，不接受审核；`ax evolution review` 提供 TUI 外同样显式决定。Trial→active 仍需成功使用。Trial/active 共用可发现目录，使用模型自主选择、弹性预算、启停及工具权限。归档立即移出发现，其他运行会话下次目录刷新／重启接收变更。分析后刷新进化指令，恢复快照移除它们，防止恢复旧归档指令。删除项保留有界元数据墓碑和追加式删除审计，Evolution 不删原会话或 Experience JSONL。

<a id="personalization-switches"></a>
## 个性化开关

观测和 worker 准备同时需要 `personalization.memory_enabled` 及独立 `personalization.evolution`。`personalization.tool_memory` 只控制工具记忆写入，不再控制学习。新安装三个默认 `true`。旧个性化配置缺 `evolution` 时继承其 `tool_memory`，保留升级前学习行为，显式 `evolution` 优先。

```bash
ax personalize --evolution false
ax personalize --tool-memory false --evolution true
ax personalize
```

`ax personalize` JSON 返回 `evolution` 及其他字段。关闭 Evolution 停新观测和 worker，`--memory false` 也同时阻止。项目配置 `enabled` 仍是额外记录／分析门槛。这些开关不删除学习包，也不禁检查、审核和归档。已有 live Skill 可经 `/skills` 禁用。

<a id="evolution-commands"></a>
## Evolution 命令

`ax evolution` 针对当前目录所属项目，子目录也适用。遵循普通存储解析，接受全局 `--data-dir`，共享目录仍由 UUID 隔离。每命令在打开 Engine 或读取／修改前，获取与 worker 相同独占 `<evolution-root>/writer.lock`，等待当前 writer，结束释放。Worker 只在准备分析及应用结果时持锁，模型请求／解析期间释放，使管理可继续。检查可在锁内执行普通恢复或旧迁移，不需提供商请求。

结果 stdout JSON；`--help` 含子命令帮助返回 `{"help":"..."}`。失败或无效参数返回 `{"error":"..."}` 和非零退出。

```bash
ax evolution status
ax evolution skills
ax evolution show release-workflow
ax evolution review release-workflow --digest <digest> --accept
ax evolution review release-workflow --digest <digest> --reject
ax evolution archive release-workflow
```

- `status` 返回 `enabled`、`pending`、`last_analysis`、`consecutive_failures`、`backoff_secs`、`pending_reviews`。`enabled` 布尔，其余非负整数；`enabled` 综合两个个性化门槛及项目 `enabled`。`pending` 为待分析数量，`last_analysis` 是末次持久尝试 Unix 秒，`0` 为未尝试。`consecutive_failures` 为 `0` 时 `backoff_secs` 为 `0`，否则是含基础冷却的当前指数重试间隔，不是剩余等待。无失败时普通冷却仍适用。
- `skills` 返回进化元数据数组，含 `name`、`state`、`use_count`、`success_count`、`failure_count`、`created_at`、`last_used_at`、`evidence_count`，省略已删墓碑，时间 Unix 秒。`digest` 是待审核摘要，无审核时包括非 candidate 为 `null`。`name`、小写 `state` 为字符串，计数／时间非负整数，`digest` 字符串或 `null`。
- `show <name>` 返回 `name`、`state`、完整 `skill_md` 及 `evidence`；每证据含 `id`、最多 200 Unicode 字符 `task` 摘要。从权威 JSONL 读取，包括近期缓冲外记录。`name`、`state`、`skill_md` 为字符串，`evidence` 为含字符串 `id`/`task` 的对象数组。
- `review <name>` 必须提供 `--digest`，且 `--accept`、`--reject` 恰选一。调用 `Engine::review_candidate`，接受进入 trial，拒绝归档。摘要不匹配、包变化或无待审核则失败，不接受。先经 `skills` 获摘要，用 `show` 阅读完整指令再决定。
- `archive <name>` 用既有所属包转换进入 archived，移出发现并清待审。审核／归档返回 `name` 及结果 `state`。归属、摘要和生命周期保护仍生效，不删原 Experience／会话。

<a id="ownership-persistence-and-configuration"></a>
## 归属、持久化与配置

全部产物按项目保存在 `<data-dir>/evolution/<project-id>/`，可移植 UUID 隔离共享自定义目录中的证据与 Skill：

```text
config.json           # optional overrides; missing fields use defaults
ledger.json           # versioned control state, byte cursors, Skill metadata
experiences.jsonl     # sole authoritative store of complete Experiences
decisions.jsonl       # action results and lifecycle transitions
writer.lock           # OS file lock; automatically released on process death
candidate/<name>/SKILL.md
live/<name>/SKILL.md   # trial and active only
archived/<name>/SKILL.md
```

RETIRE 不需创建证据仍在窗口，归档核归属／摘要，删除仍需后续 epoch 再显式 RETIRE。分析包只含 Candidate、Trial、Active 正文，归档元数据仍可用于退役。MEMORY/REFINE 原文来源至少八个去首尾空白 Unicode 字符或四个 CJK，且有字母／数字；仅空白标点不成立。

`experiences.jsonl` 为权威。记录先验证范围／秘密，追加完整不变 `Experience` 并同步，再保存控制。版本 2 账本含 `version`、`epoch`、`skills`、`pending`、`last_analysis`、`observed_cursor`、`processed_cursor`，不含 Experience 正文或近期副本。`observed_cursor` 连同计数保存使用遥测，`processed_cursor` 保存成功消费，均是 JSONL 字节边界。增量 `trial_requests`、`failed_batch_cursor`、`failed_batch_attempts`、`consecutive_failures` 保存审核及恢复调度。每新 Experience 读取新字节并仅写一次账本；重复且无未检查点字节不重写。旧 `Experience.intent` 可读，不序列化或发送分析。

Worker 跨命令保留 Engine。重启首用流式扫描一次 JSONL，重建内存 ID／偏移索引及有界近期缓冲。索引只含 ID／偏移，随唯一观测数增长。后续仅索引追加字节。分析 seek 到下一有界待批，容量允许保留前上下文。适配上下文不能丢最早待观测又越过它；排除的新观测继续等待。超过 `max_experiences` 的积压按冷却分批消费，不静默丢旧工作。

解析逐个尝试左方括号，取首个可解析 JSON 数组，容忍带括号说明、代码块及尾文。逐项审计／跳无效动作，超过 `max_actions` 截断并审计。动作应用、政策拒绝审计及维护后正常推进游标。无效模型响应按待批起始 processed 游标持久计数，新观测不重置；同游标三次无效后，先审计跳过，再推进该批末游标，原数据保留。超时及提供商／网络错误只增加持久退避，不算跳过次数、不推进。审计、文件、Memory、检查点失败保留待工作，不能未审计跳过。请求前保存尝试时间，连续失败跨重启使冷却翻倍，最高 24 小时，成功重置。默认请求超时 120 秒。

准备在 `writer.lock` 内快照选批末游标及起始 `processed_cursor`，保存 `last_analysis`，请求／解析不持锁。应用前重获锁并刷新 Engine，保留并发观测及管理变化。如 `processed_cursor` 已变，丢结果，写 `ANALYSIS_SUPERSEDED` 审计及 `analysis superseded`，不应用、维护、消费或改该过期尝试失败／退避计数。仅 JSONL 追加可恢复一次遥测，窗口外及跨重启重复 ID 也抑制。

Skill 文件、Memory、审计、账本不构成单事务，动作后游标前崩溃可能重分析。归属／摘要与乐观写入保护目标，这是至少一次分析，不是跨存储恰一次。CREATE 账本失败删除刚写包。MERGE 预检全部来源归档目标，批量移动后一次检查点；普通移动／检查点失败恢复来源、移除新包，不声称多文件崩溃原子性。

旧迁移在 worker 持 `writer.lock` 时进行。已有 JSONL ID 不重追加，仅旧账本有的才追加；副本不同以 JSONL 为准。旧 pending 后缀映射字节游标，包含旧检查点失败留下仅 JSONL 追加。保留 Skill 元数据、使用计数、epoch、冷却。先在临时 `experience_migration` 保存原字节边界，再原子存不含 `experiences` 或临时字段的 v2。中断按 ID 及原边界重试。先前已消费但缺副本记录追加于 pending 后可能保守重分析，已有待工作不跳过。仅旧账本的派生纠正不覆盖权威原观测，生命周期纠正计数保留。旧 AX writer 期待已删 `experiences`，不可用迁移后存储。截断、格式坏、非边界游标或撕裂末行拒绝继续，不重写原数据，需备份恢复再学习。

元数据独立于 SKILL.md：`source`、`created_at`、`last_used_at`、`use_count`、`success_count`、`failure_count`、`corrections`、`confidence`、`state`、项目归属、内容摘要、证据 ID 和审核基线。仅 `source=evolved`、项目匹配、摘要未变可自动改。内置／用户根优先，绝不为变更目标，已知非进化名称也排除提案。变更区域拒绝符号链接及 Windows reparse，人工编辑停止该包自动变更。

记忆学习初期限 Project，复用键值／凭据验证和乐观检查，不覆盖用户事实，引用来源须在选中证据。Experience/Skill 中已识别疑似凭据经现有 Memory 筛查排除，过滤保守非完整秘密检测。私有学习与会话有相同本地隐私考虑，仅合格分析发送选定 AX 提供商。Evolution 尚不包含在 `.axpack`，普通已学事实按原记忆导出。

模块为 `config.rs`（限制）、`types.rs`（经验／账本／提案）、`engine.rs`（唯一变更边界）、`policy.rs`（引擎执行的生命周期决策表）、`storage.rs`（原子写、摘要、区域保护、秘密筛查）、`worker.rs`（启动路径外分析线程），`lib.rs` 仅重导出。

活跃调度和增长设置在 `evolution::Config`，例如：

```json
{
  "enabled": true,
  "batch_size": 12,
  "cooldown_secs": 1800,
  "analysis_timeout_secs": 120,
  "max_experiences": 64,
  "max_skills": 24,
  "max_retained_skills": 48,
  "max_tombstones": 24,
  "max_skill_bytes": 12000,
  "max_actions": 4,
  "minimum_evidence": 2,
  "independent_sessions": 2
}
```

旧权重曾代表重复、成功、近期使用、纠正稳定、项目相关和置信度，现不参与决策。项目相关仅保守归属，不跨项目泛化。批量／冷却是调度，非生命周期规则。Live 上限计 candidate/trial/active，另 retained-package 上限计它们及 archive 总大小，满时只能细化／压缩和退役腾空间。归档可审阅至效用支持删除，墓碑独立有界，完整删除元数据留审计。生成／细化正文受大小预算，合并还必须缩减总来源。无效配置拒绝并诊断；`enabled=false` 关闭记录分析，live Skill 仍可 `/skills` 禁用。每观测 worker 重读配置。

单元／集成覆盖归属、标准格式、路由、trial 晋升、纠正／重试用计数、合并压缩、归档删除顺序、记忆来源、冷却、禁用、秘密筛查、上下文适配、调用频率及分析失败。持久化还覆盖旧迁移、仅 JSONL 恢复、增量读、有界积压、窗口无关去重、失败游标／审计、重启消费及撕裂／截断流。

旧 `similarity`、`create_score`、`trial_score`、`active_score`、`retire_score`、`half_life_secs`、`weights` 仍可读，不决定语义创建、晋升或退役。保留归属账本和计数，不需数据库迁移或向量库。分析输入完全省略 Config，旧评分仅反序列化兼容，非模型输入或生命周期策略。见 [ADR 0034](adr/0034-reviewed-evolution-and-analysis-recovery.zh-CN.md)。
