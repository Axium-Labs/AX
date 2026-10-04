## 2026-10-03

Task: 持续执行通用 Coding Agent 长任务，修复原始 testax.txt 黑盒运行发现的运行时问题。

Changed:
- core/harness、loop_runtime、task_queue、child/child_dispatch/child_result、execution、kernel：共享 policy/environment，typed inventory 自动建队列/追加，streaming completion review，pending guard，task WorkspaceSpec、setup 失败唯一 receipt、统计与 trace、harness step scope advisory。
- cli/child_runtime、child_workspace、runtime/builder、skill_invocation：共享入口 host/context，Git cache/revision/worktree/subdir，冻结 patch/artifacts 后清理，staging export，允许已索引 global skill 的 runtime-owned 读取。
- tool/environment、task_source、sandboxed/workspace：懒缓存环境探测、投影 parquet/JSON/CSV、完整 record-to-task 映射、精确 revision 与顺序显式选择、child cwd/root 分离。
- test/harness/、既有 core/CLI 测试：23-item activation/guard、streaming-only provider、append、局部 setup 恢复、两仓库/revision/subdir、partial failure、durable artifacts、CLI/ACP 自动 dispatch。
- docs/coding-harness.md、architecture/context/tools 与 docs 索引：同步真实行为。

Why:
- text-only response 会提前阻断未完成 queue；无 queue 时缺少 final review。
- Codex provider 要求 streaming，最初独立 completion 调用遭 HTTP 400。
- global skill 外部目录遭 workspace scope 拒绝；无实际证据的 model block 假冒全局停止。
- setup checklist 已执行后不能注册真正发现的 23-item 工作；追加需保留历史。
- Child 原本只支持 controller repo，缺 task repo/revision 准备与 durable patch/export。
- task_source 相对 output 与 host 绝对 path contract 不一致。
- 模型 step.scope 猜出 workspace/workspace 后锁住正常读取；harness 下改 advisory，初始 workspace 边界仍强制。
- record-to-repo 映射遗漏 revision 会静默取 HEAD；现在要求显式 revision source 和顺序选择。

Tests:
- workspace tests：多轮 PASS；最新完整最终检查待当前黑盒终态后确认。
- test/harness/core.rs、task_source.rs、workspaces.rs：PASS（包括真实 local Git、shell/edit、staging 与 cleanup）。
- cargo fmt --all：PASS。
- clippy --workspace --all-targets：exit 0；正在收尾新增 lint，保留既有未相关 warning。
- 原始 testax SHA256: 14522F3E555F0EA98CB2F1E0B15F547019F029FEB6F5799F6362D40D5D50A6EB。

Blackbox facts:
- 每轮使用原样 testax.txt、独立 data-dir/session、codex/gpt-6-luna/low、900s child timeout；AX 自行读取、注册、准备和解题，未人工执行 instance 修复。
- 早期尝试通过日志定位：HTTP403 proxy、streaming HTTP400、skill scope、假 block、queue 替换、relative output、step scope、omitted revision。
- 07/08/09 已实际进入多个 instance，并出现修改/测试/局部失败继续；为修复明确 runtime 原因主动停止后重建重跑，不能算最终验收。
- blackbox-10 原始 23 项 terminal（21 completed/2 failed），但缺 title/output 的 summary/review 信息导致报告阶段误判、错误追加恢复；该轮不作最终验收。
- 最新 blackbox-11 运行中。读取 task_source 仅四个用户允许字段，映射明确 repo URL 与 base_commit revision。后续补终态数量及最终报告位置。

Issues/Next:
- 等待 blackbox-11 全部 23 项 terminal 与报告；若发现 runtime 问题，继续定位、回归、重建重跑。
- 没有自动提交、发布，也没有改 AXCrew、testax.txt 或历史日志。
- 工作区原有 .gitignore/AGENTS/docs README/acp-crew diff 已保留。测试二进制/session/log 在本地 git info exclude 中排除；回归 .rs 与文档保留为待审代码。

Additional changes:
- controller/child 共用 completion review，审查可以请求普通工具动作；接受审查后的 child receipt recovery 保留原 final 与 unresolved tool error。
- summary/review 同时包含全量 terminal title、task_id、workspace/revision、output_dir，防止把 failed 误当成未执行项。
- LocalChildHost 也解析 direct task_queue 的 relative output_dir；GC 保存并复用原 source write capability。
- test/harness/frontend.rs：CLI/ACP typed inventory 自动 dispatch 23 children；新的 core summary review 与 child audit recovery 回归通过。
- 单次 workspace test 曾碰到既有 WorkBuddy timestamp 临时凭据文件冲突（icacls 找不到 .tmp）；单项重试及完整重跑通过，未修改 model/auth 生产逻辑。

Additional durability/measurement regression:
- 新增 child_file_baseline：非 Git empty/inherit workspace 准备时记录内容指纹，捕获不带文件事件的 shell 编辑，cleanup 前保存实际变更文件。
- DiffStat insertions/deletions 可空；binary/unavailable Git counts 不再伪装为零。丢失 round usage 时 aggregate measured tokens 为 null；staging 输出不计 first code edit。
- 首轮已公布 child_result，保证首次 dispatch 后可在同一 Goal 查询 receipt。
- test/harness/workspaces.rs 新增真实非 Git shell 修改/cleanup/relative output 回归；core.rs 新增缺失 usage 与 staging-edit timer 回归。
- 完整 workspace tests：534 passed / 0 failed / 3 ignored。clippy exit 0；新增堆上读缓冲区消除 large_stack_arrays warning，未改既有无关 warnings。
- 最新二进制构建 PASS；blackbox-11 仍在自主顺序执行，结束后以更新后的构建开始独立最终验收。

Blackbox continuation:
- blackbox-11 最终 23 durable receipts：17 completed / 6 failed / 0 timeout，controller 因原生 provider HTTP429 usage_limit_reached 结束，总报告缺失，不作为验收通过。
- 日志包含 provider reset UTC 2026-10-03 09:45:27；当前限额查询已重置，已启动 blackbox-12，最新构建、独立 data-dir/session、原样 testax、同一 codex/gpt-6-luna/low。

TaskSource compatibility fix:
- blackbox-12 真实日志：模型把 work 对象编码成 JSON string，task_source 误报 title_column required；模型随后错误进入 WaitingForUser，没有开始 23-task 执行。
- task_source 现在解码明确 JSON object string；其他形状返回准确可恢复诊断。投影列、revision、sequential 约束保持一致。
- test/harness/task_source.rs 验证 object/string 生成完全相同 inventory，非法数组形状被拒绝；tool task_source tests PASS，clippy exit 0。
- 重建后用独立 blackbox-13 再执行原始任务。

Output freshness fix:
- blackbox-13 第 2/5 题 output result.json 仍为上一轮 08:19/08:24 UTC 文件，而当前独立 state receipt 为本轮；根因为 host 以目标文件存在决定保留 custom result。
- 仅当前 staging 生成的 result.json 被作为 custom 输出保留；否则写本轮 receipt，避免失败/无自定义结果时遗留上一轮结果。
- 每个导出的当前 staging report 注册 durable Artifact 路径，controller 可按当前 receipt 查询本轮产物。
- test/harness/workspaces.rs 添加旧 output result 替换和当前 evaluation artifact 引用断言；非 Git 回归 PASS；完整测试/黑盒收尾继续。

Report-stage recovery:
- blackbox-13 原始 23 项 terminal：17 completed / 6 failed / 0 timeout。多个不同 repo 实际 edit/test；Git prepare 网络失败独立 receipt 后继续下一项。
- 模型反复把跨任务汇总派给 empty workspace collector，没有提供报告输入；glob 找不到本地 axout，scope/编码错误反复恢复，总报告未生成。
- 终态摘要明确 controller 使用 child_result 读取本轮权威 receipt 并自己生成跨任务报告；隔离报告 child 必须具备完整输入。仍为模型执行指导，没有新增 progress/recovery hard lock。
- core completion review 回归验证全量 terminal identity/output + authoritative receipt/controller reporting 指导；PASS。
- 已确认并停止本轮所属 PID 25796；不作为最终验收。最新构建启动 blackbox-14 干净数据/session、原样任务、同一模型低模式。

Prepared-workspace context fix:
- blackbox-14 task-3 的 host WorkspaceSpec 已正确 git/revision，但 child 又执行 git clone ... repo; checkout requested commit，编辑嵌套 repo/pvlib/temperature.py。Child context 只有 cwd/shell，没有明确准备完成的 repo/revision，重复 clone 会影响 host 根仓库 diff 捕获。
- [ax-task-workspace] 注入 prepared=true、WorkspaceSpec/cwd/workspace_root 和直接在已准备 checkout 编辑验证的指示；empty workspace 明确不包含 sibling/controller 输出。没有新增恢复硬锁。
- 真实两仓库回归在 child 第一次请求验证 mode/revision/prepared metadata，随后真实 git rev-parse 验证 exact commit；PASS。
- 已停止已核验本轮 PID 21812，clippy/build PASS；完整 tests 和最新 blackbox-15 验收继续。

Child deadline fix:
- blackbox-15 已恢复初始错误 inherit/checklist 注册，随后自动登记原始 23 项；workspace prepared context 避免重复 clone，旧 result 覆盖也实测正常。
- 第 6 项反复写 incomplete artifact，turn_timeout_secs 复用 controller 的 idle 语义，每次活动续期；显式 --child-timeout-secs 900 未提供实际总执行上限。
- isolated child 显式 timeout 改为总 model/tool 执行时限，含 completion review；controller 保留 idle 语义。该预算不是 observed-progress/recovery gate，0 仍无限制。
- test/harness/child_deadline.rs：模型每 100ms 继续工具调用，1s 时限仍 TimedOut，下一独立项 Completed；PASS。既有 productive controller batch outlives idle window 保留回归。
- 已核验并停止本轮 PID 30988；clippy exit 0，完整 tests/build 和 blackbox-16 干净重跑继续。

Completion recovery window fix:
- 代码复核确认：普通 final candidate 在审查前 checkpoint；terminal_result 对 raw text 原本直接生成终态，进程在审查中断可能把未经审查的 candidate 恢复为成功。
- [ax-completion-pending] 在候选前写入 raw checkpoint；恢复未接受的 candidate 返回 None，继续原 child。Accepted completion_check 可恢复候选，同时保留 unresolved tool failure；无 pending marker 的 legacy history 行为保留。
- core 回归验证 pending marker 在 candidate 前 checkpoint，未接受时不生成 receipt，接受时恢复终态且不掩盖原失败；PASS。
- 已核验并停止 blackbox-16 PID 37692；构建/clippy PASS，完整 tests 和 blackbox-17 最新原样验收继续。


## 2026-10-03 22:00 — Final report evidence recovery

- blackbox-17 original 23 items all terminal: 23 completed / 0 failed / 0 timeout; actual repository edits and local validation across multiple instances. AX produced summary.csv/summary.json/report.md and exited 0.
- Report incorrectly classified six custom status labels as failed/non-completed; final answer cited an older quota failure. Some evaluation files were historical, and absent error classifications became zero in the report. This is not accepted as final validation.
- Root causes: terminal inventory injected only in Summarizing, not Completed; report guidance lacked current custom-artifact provenance and canonical status distinction.
- Added generic artifact-manifest.json, authoritative receipt/status guidance, null measurement guidance, and terminal context after Completed. No benchmark-specific production logic or output rewriting.
- Regression now reviews both Summarizing/Completed; non-Git workspace test seeds stale evaluation and verifies it is absent from the current manifest. Full tests/build and blackbox-18 clean rerun pending.


## 2026-10-03 22:55 — Provider quota reset retry

- blackbox-18: 23 original tasks terminal, 5 completed / 18 failed / 0 timeout; all 18 failed receipts contain native codex HTTP429 usage_limit_reached. Controller could not generate this run's report. No acceptance claim.
- Provider reset timestamp 1791038882 = Asia/Shanghai 2026-10-03 22:48:02; current clock verified 22:55, past reset.
- fmt/clippy/build PASS, workspace 535 passed / 0 failed / 3 ignored after current evidence fixes.
- New clean blackbox-19 uses the same latest binary and original testax as one complete prompt, native gpt-6-luna/low. No manual per-instance driving, no benchmark answers read. Acceptance is durable requested output/report completeness, not all issues resolved.
- New workspace site instructions reviewed: generic agent/runtime descriptions need no release/version/website changes for this unpublished AX change. AXCrew untouched.


## 2026-10-04 00:23 — Output manifest completeness

- blackbox-19 clean whole-prompt run exited 0; original 23 terminal: 20 completed / 3 failed / 0 timeout. AX generated fresh summary.csv/summary.json/report.md, canonical counts correct and unknown evaluation/error fields null.
- Acceptance still incomplete: report listed missing files; artifact manifest omitted host-generated result/trace/metrics although files existed, while some children did not produce evaluation reports.
- Manifest now includes all host outputs. Preparation failure also writes canonical receipts/metrics/diagnostics/manifest in requested output. Terminal summary/review explicitly requires requested reports for failed and completed items; missing reports must be written from current evidence with null unknowns, not merely listed.
- Regression asserts host outputs are current manifest entries and old evaluation remains excluded. Full fmt/clippy/test/build and clean blackbox-20 pending.


## 2026-10-04 01:55 — Direct current receipt evidence

- blackbox-20 original 23 terminal: 20 completed / 3 failed / 0 timeout; two extra registration-recovery helpers failed snapshot quota and are not benchmark instances. AX exited 0, but only report.md was refreshed, summaries remained blackbox-19 and requested per-item reports were still missing. Not accepted.
- Current receipts were available through child_result tools but not injected directly into summarization/review. Added compact authoritative status/metrics/diff/current output filename index to both request paths, and registered host output references in ChildResult itself.
- Summary review regression now asserts direct current receipt evidence in both Summarizing and Completed; full tests pending.
- Entire old axout was preserved by validated native Move-Item to test/harness/blackbox-20/previous-axout; new axout is empty. No benchmark output manufactured or historical files discarded. This prevents stale aggregates from satisfying existence checks in the next full-prompt blackbox-21.


## 2026-10-04 03:20 — Whole-prompt acceptance completed

Task: Complete generic coding harness repair and original testax blackbox acceptance

Changed:
- core harness/task queue/current receipt context; cli workspace/artifact host; tool environment/task_source; test/harness regression and current behavior docs (full module/root cause report in docs/coding-harness-validation.md).

Tests:
- Final cargo fmt --all PASS; cargo clippy --workspace --all-targets exit 0 (existing warnings); cargo test --workspace 535 passed / 0 failed / 3 ignored.
- blackbox-21 latest built AX, native codex/gpt-6-luna/low, clean data/session and empty axout, original testax supplied as ONE complete prompt: exit 0.
- Original 23 terminal = 19 completed / 4 failed / 0 timeout. One extra duplicate registration helper failed snapshot quota, excluded from benchmark.
- AX generated 23/23 result.json/trace.jsonl/final.patch/evaluation.json plus fresh summary.csv, summary.json, report.md. CSV/JSON 23 rows, statuses match current authoritative receipts; official_resolved all null.
- 22 nonempty authoritative patches, multiple repositories edited and locally validated. One empty patch does not establish correctness.
- Original testax SHA256 remains 14522F3E555F0EA98CB2F1E0B15F547019F029FEB6F5799F6362D40D5D50A6EB.

Issues:
- Four original failures: task-3 local/tool verification, task-8 invalid result dependency, task-13 unresolved tool failure, task-17 model response decode. No official evaluator executed.
- Final AX prose mentioned quota; this belongs to the extra helper, not the original 23 failures. Authoritative table and summary use correct records.
- Nonblocking Evolution experience request still logs native provider HTTP400 Stream must be set to true; coding/report run exits 0. No release/commit or AXCrew changes.
- Advisory semantic review is not a universal model correctness guarantee; live platform Windows PowerShell/sandbox off.

Next:
- No required work remains for this acceptance. Future official evaluator/model quality work is separate; retain blackbox-21 logs and previous-axout archive for reproducibility.
