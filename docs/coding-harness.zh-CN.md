<a id="coding-execution-harness"></a>
# 编码执行 Harness

[English](coding-harness.md) | **简体中文**

Coding harness **就是**运行时。CLI、TUI、ACP、Crew 桥接及每个隔离子执行，都在 coding 策略下，以完整环境快照和每 Goal 队列执行用户交付物。没有模式开关或配置标志，唯一变化是 Host 注册哪些工具和提供商。契约与触发条件见 [agent-runtime.md](agent-runtime.zh-CN.md)。

每次运行获得含可执行探测的完整环境快照、建议性步骤范围、coding 策略（`[ax-coding-harness]`）及每 Goal 队列行为。各前端共用 `AgentKernel`、工具注册表、`LocalChildHost`、执行预算和检查点路径，编码能力没有界面差异。

<a id="policy-and-completion"></a>
## 策略与完成

Harness 中 `[ax-coding-harness]` 指导执行至交付物完成、附证据失败，或因只能由用户决定的事项挂起。缺 runner、venv、可选包、pytest、未克隆仓库和无匹配搜索均为可恢复初始化观测。应尝试安装、创建或回退，记录局部失败，继续独立工作。先发现具体条目再入队，不用环境发现清单替代交付物。使用选定数据集列及隔离完整子输入。官方 evaluator 不可用则 `official_resolved=null`，不妨碍编码和本地验证。清理前保存补丁及报告。

这是模型建议策略，不是观测进度门槛或恢复锁。模型步骤子范围仅建议，工具仍执行初始工作区边界，误写 `workspace/workspace` 不会阻止检查真实 checkout 或在边界内恢复。控制器和子执行都按续接状态完成。最终文本立即流式输出；`TurnState::continuation()` 仅在没有待执行、结果消费、任务、子代理、审批、重试、输入或下一动作时允许完成。成功工具、编辑、命令和测试本身不触发额外模型请求；普通回答一次执行请求、零审查，无复杂度分类器。

独立验证是默认关闭的可选 `Goal`，确定性检查先于显式开启的模型验证。配置、事件、扩展及恢复见 [agent-loop.md](agent-loop.zh-CN.md)。旧完成审查标记仅用于兼容恢复，默认新轮次不写。子最终响应仍有未解决工具错误时返回失败回执。

<a id="evidence-review-after-tool-errors"></a>
### 工具错误后的证据审查

当前主机轮次出现类型化可恢复错误后，首个有效 `request_user_input` 得到 `review_required` 而不打开问题。它收到普通工具结果，不挂起，原模型继续，可尝试文档替代方法或再次提交必要问题。命令缺失或一个客户端未登录，不证明所有支持接口不可用。应经预期接口复用已有授权，不暴露或持久保存秘密。

最新工具轮次仍含此错误时，首个普通最终响应也获一次 `[ax-handoff-review]` 续接，核对声称结果、回执、文件和验证。后续恢复／检查成功或队列结果已记录，则直接完成，不仅因旧失败再审。可用工具恢复或凭充分已有证据结束。这是有界自审，不是独立 evaluator 或成功保证。每主机轮次最多一次提问审查和一次最终审查，更多错误不增加次数。新主机轮次，包括回答／恢复，重置额度。当前原回执跨压缩保留，问题边界从当前历史抽取、最终边界从最新工具轮抽取，最多八个失败 ID。

旧轮次错误、非结构化错误文字、真实类型化权限拒绝及全局阻塞不触发。启用 Goal 时最终检查由它负责；有限模型步预算已耗尽也不额外审查。权限、取消、队列、补充指令规则优先。审查时流式候选仍可见，`TurnFinished` 标志接受最终答案；候选前 `[ax-handoff-pending]` 防止中断恢复误认子完成。

编辑指引优先原子补丁、项目脚本或格式感知更新，依赖步骤前检查修改区域；将编辑后错误与编辑前对照，再归因仓库。最终报告区分尝试、修改、构建、验证和外部交付。来源与权衡见 [ADR 0029](adr/0029-evidence-driven-handoff.zh-CN.md)。

<a id="concrete-work-activation"></a>
## 激活具体工作

`task_source.work` 接受 schema 对象及模型调用显式 JSON 编码对象，其他形状返回可恢复输入诊断，解码不改变投影、修订和顺序要求。

`task_source` 读取显式选定 parquet/JSON/CSV 列，parquet 在物化行前列投影。`work` 映射提供标题列、完整逐项指令、可选仓库 URL 模板／修订、输出根及显式顺序资源。仓库映射必须选明确共同修订或修订列；静默把所需 commit 默认 HEAD 会被视为可恢复输入错误。返回显式类型化信封：

```json
{"ax_work_items":[{"title":"item","input":"complete standalone instructions",
 "workspace":{"mode":"git","repo_url":"https://example/repo.git","revision":"commit"},
 "output_dir":"absolute durable path"}]}
```

任何成功工具都可返回它。控制器在模型上下文投影前接纳，并按相同 title/input 去重。新清单默认控制器执行，数据映射不选择委派。模型可在有益且可用时选 `task_queue action=configure, execution=children`，保留标识、依赖和回执。追加记录在现限制内保留已有显式执行选择。普通项目符号、编号规则和任意表格行不解析为任务，也可直接 `task_queue start` 创建具体项。`append` 追加新发现项并调整依赖偏移，保留执行历史、子回执和 ID，不能抹去旧工作。

```text
Goal active → projected concrete inventory → start/append controller queue
→ model selects direct execution or configures children → ready work
→ task-local receipt → next independent ready item → all terminal → model report
→ completed
```

顺序映射用共享写资源，不用成功依赖，首项失败不会跳后项。相对输出根按绑定执行目录解析。JSON/CSV 本地解析并返回选字段，parquet 除非明确请求不加载答案列。此通用工具不强制基准专属字段白名单，投影由调用者输入契约决定。Evaluator 是冻结补丁后的独立执行活动。

<a id="workspace-lifecycle"></a>
## 工作区生命周期

`QueuedTask.workspace` 是 `WorkspaceSpec { mode: inherit|git|empty, repo_url?,
revision?, subdir? }`，旧快照默认 inherit。可选 `output_dir` 指持久输出。Inherit 用过滤控制器快照／脏覆盖，empty 创建隔离目录。Git 在任何子模型请求前准备：验证、配额准入、取得 URL 标识 bare 对象缓存、解析／fetch 修订到 commit、detached checkout。Off 模式轻量 worktree，受限模式独立对象存储，不共享可写 Git 元数据。生命周期命令关闭 hook/fsmonitor，各子有独立会话和记忆作用域。

`ChildRun.cwd` 是所选子目录，可选 `workspace_root` 保留生命周期／沙盒／补丁范围。相对工具路径及 shell 目录用 cwd，隔离和最终补丁捕获用仓库根。旧快照无 workspace_root 则用 cwd，拒绝子目录遍历及逃逸符号链接。准备失败产生唯一失败回执，可写请求输出，不阻止其他独立项。

<a id="environment-and-errors"></a>
## 环境与错误

各主／子执行接收紧凑 `[ax-environment]` JSON。可执行路径和有界版本探测惰性缓存于进程，cwd／根／沙盒按次重绑定。Windows 示例：

```json
{"os":"windows","shell":"Windows PowerShell 5.1 (powershell.exe)",
 "shell_contract":"Use PowerShell here-strings or python -c; no bash heredoc, && or ||. Use separate commands and check $LASTEXITCODE.",
 "cwd":"C:/runs/item/workspace/pkg","workspace_root":"C:/runs/item/workspace",
 "path_separator":"\\","executables":{"git":{"path":"C:/.../git.exe","version":"git version ..."}},
 "network_policy":"sandbox allows network; tool permissions still apply",
 "sandbox":"Off","write_boundary":"C:/runs/item/workspace"}
```

可执行缺失则省略，版本失败为 null；这是启动能力摘要，不是实时包清单。修改程序／PATH 后可能需新进程。检测早于轮次空闲计时。

普通工具／初始化错误保持可恢复或任务局部。Harness `task_queue block` 要求 `evidence_call_ids` 引用带类型化 `global_blocker` 的真实工具信封，文字不能虚构全局停止。提供商耗尽、持久化失败、取消和全局预算保留实际内核失败／挂起路径。必要用户决定使用结构化 `request_user_input`，保存 WaitingForUser 后恢复原 Goal。

子上下文还含 `[ax-task-workspace]`，提供准备的 WorkspaceSpec、cwd 和根。Git 子代理获知已在所需修订 checkout，应编辑现有工作区，不克隆嵌套副本。这是执行上下文，不是禁止恢复工具的运行时规则。

<a id="durable-artifacts"></a>
## 持久产物

成功／失败清理前，host 捕获权威 Git status 和 binary/full-index 补丁，包括普通新文件，复制变更及声明产物，序列化诊断／验证／指标和详细 trace，再同步原子替换及保存 SQLite 持久回执。状态在可丢弃工作区外，保存失败不得清理。GC 也冻结无租约到期工作，并读取已存输出目标。

子在 `.ax-artifacts/` 写请求自定义 JSON／报告，host 将常规暂存文件导出至状态及已验证输出根。未跟踪补丁排除运行时暂存、venv、常见构建／缓存目录，已跟踪修改始终保留。`child_result.json` 权威，当前暂存目录产生的 `result.json` 保留到输出；无当前自定义结果时，当前回执替换旧输出结果。暂存报告列为持久产物，实测指标／trace 为 host 所有。回执包含持久路径，不指已删除工作区。未知提供商 Token 在可选实测字段为 null，旧计数兼容。中断在途 trace span 以实测时长及 interrupted 关闭，不虚构错误分类。

非 Git `empty`/`inherit` 在执行前记录内容指纹，清理前比较，也能捕获没有文件事件的 shell 编辑。变更内容跨清理保留，无 Git 基线时 `final.patch` 为空并明确诊断。新增／删除数可 null，包括不可用或二进制 Git 统计。保留轮次缺模型用量则聚合实测 Token 为 null，不报告部分和。写暂存报告不算首次代码修改。控制器首分发前公布 `child_result`，使同 Goal 可检查新回执。

配置子超时限制整个模型／工具循环，包括显式可选 guard 评估，活动不能续期。控制器仍空闲超时，生产性独立批次可更长。超时子保存回执／产物，其他独立任务继续。

<a id="regression-coverage"></a>
## 回归覆盖

`test/harness/core.rs` 覆盖类型化动态 23 项准入、不解析项目符号、未完成续接、直接完成、已执行队列追加、共享环境绑定和局部／全局证据分类。`test/harness/task_source.rs` 覆盖投影输入隔离、23 项映射、顺序失败续接及相对输出。`test/harness/workspaces.rs` 准备两个不同精确修订的本地 Git 库，核实子目录 cwd，先经历初始化失败再执行真实 shell／编辑，验证清理后持久补丁／回执和来源隔离；还比对非 Git shell 基线、运行目录下保留及可空 diff 数。`test/harness/child_deadline.rs` 验证连续活动不刷新显式子截止，下一独立项仍完成。既有 CLI 分发测试覆盖 CLI/TUI/ACP 子执行，内核覆盖真实用户提问挂起／恢复。这些是 harness 测试，不是基准专属规划器或正确性 oracle。

<a id="current-output-evidence"></a>
### 当前输出证据

每份冻结输出的 `artifact-manifest.json` 标识当前子／任务及当前导出产物。未列出的已有自定义文件可能是历史，不能用于当前评估。终态归 `child_result.json`，自定义 `result.json` 标签仅描述。跨项报告用实测 `metrics.json`，未知为 null，按终态清单核总量。队列 completed 后仍注入终态清单，包括可选验证阶段。

当前清单即使无子暂存也含 host 回执、trace、补丁、指标、验证和诊断。初始化失败发布同样回执／清单。策略要求失败和完成项都提供所需报告；测量／评估不可用为 null，不免除文件。

当前子状态、实测指标、diff 及分组输出文件名直接注入控制器终态摘要和可选 Goal 验证，无需模型先发现／调用 child_result。Host 输出既为 Artifact 引用，也为清单项，提供执行上下文，不增加自动语义审查。
