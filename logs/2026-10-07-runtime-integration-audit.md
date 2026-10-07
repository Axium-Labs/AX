# 2026-10-07 — Agent Runtime integration audit (wiring verification)

Task: verify that the neutral runtime, capability guidance, loop hygiene and
task/delegation gating from 2026-10-06/07 are actually wired into the
production `AgentKernel -> loop -> ModelRequest` path. Audit first, repair only
where wiring was missing.

## Audit result (all production paths confirmed wired)

- `prepare_environment` is called once per turn at
  `loop_runtime/mod.rs:102` (`run_goal_turn_checkpointed`); it injects
  `[ax-agent-runtime]`, `[ax-delegation]`, `[ax-capability-guidance]`,
  `[ax-environment]` via `set_context` (replace-in-place: no accumulation, no
  loss). `model_step` builds `request_messages` from `self.messages`
  (`request_context`), so every turn's `ModelRequest` carries all four
  sections. `request_pool`/`select_context` keep them (History pool) and
  compression preserves all System messages as persistent context.
- Coding harness: only `config.harness.enabled` (CLI `builder.rs:157`) or an
  explicit `with_coding_harness()` call sets it; `HarnessConfig` defaults off
  and is omitted from persisted config; `begin_goal` creates no queue for a
  neutral kernel (`had_queue || coding_harness`); children inherit via
  `fork_with_messages`, never force it. No environment heuristic touches the
  flag. `tool_policy.md` is gone from source (only stale copies inside
  `target/` build artifacts remain).
- Loop hygiene: `repeat_calls.reset()` per turn (`loop_runtime/mod.rs:101`);
  `tool_step` observes every executed call after the round and appends an
  advisory `[ax-loop-hygiene]` reminder (tool_step.rs:397-409). New test
  `g_every_repeat_threshold_fires_in_the_real_loop` drives 8 identical calls
  through the real loop and asserts gentle@3, detailed@5 and detailed@8, no
  blocking, all calls executed.
- `Tool::guidance`: trait default `None` (tool/lib.rs:140); built-ins
  (shell/ssh/filesystem/search/find/patch/web/task_source + sandboxed
  delegation) contribute capability-scoped usage text; none contains
  "use this tool / always / you should call". `harness.rs::capability_guidance`
  assembles them in tool-name order into `[ax-capability-guidance]`.
- Task/subagent gating: task_queue tool description requires "genuinely
  multi-item or long-running"; queue admission needs >= 2 items;
  `SubagentConfig` defaults `enabled:false` and `prepare_subagents` registers
  the `subagent` tool only when enabled; project instructions render as
  "supporting context … never override the current user request".

## Changed

- `test/harness/runtime_neutrality.rs`: added
  `g_every_repeat_threshold_fires_in_the_real_loop` (3/5/8 reminder
  verification through the real production loop; reminders persist in the
  transcript, asserted by content and first-appearance position).

## Real-binary behaviour verification (isolated `AX_HOME`, DeepSeek
`deepseek-flash`, debug build)

- A "1+1等于多少？直接回答即可" in an empty dir: answer "2", 1 model step,
  `tools: 0`, completion `direct`, no queue. Events: turn_started ->
  model_started -> deltas -> continuation complete -> completion -> finished.
- C "鲁迅有哪些代表作品？" with cwd = axlab (code-heavy workspace): direct
  knowledge answer, `tools: 0`, no workspace scan, no queue.
- D-lite "读取当前目录下的 a.txt 并告诉我里面写了什么": exactly one
  `filesystem read` tool call, 2 model steps, 1 tool, correct content, no
  queue — escalation ladder behaves step by step.
- `ax settings` on the real binary shows subagent defaults
  (enabled=false); a real run persisted no `harness` key into config.json.

## Environment findings (not code regressions)

- Running a dev binary from `target/debug` without `AX_HOME` makes
  `ax_home()` resolve to `target/debug/.ax`, and `migrate_home` then copies
  the user's real `~/.ax` there. Because `~/.ax/projects/ff646…` already holds
  1.4 GB of nested child-run repo copies, each killed run restarted a
  multi-GB copy with no marker — this looked like a startup hang. Verified
  with stage instrumentation (since reverted) and avoided in verification by
  setting `AX_HOME` to an isolated dir. Suggested follow-up (not done here):
  consider a size guard or progress marker for `copy_missing`.
- Test runs normalised `~/.ax/config.json` to the full schema (model section
  preserved, no semantic change).

## Tests

- `cargo check --workspace --all-targets`: PASS
- `cargo test --workspace`: PASS (30 binaries, 587 tests; runtime-core
  neutrality suite 11/11 including the new threshold test)
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS
- Pre-existing failures: none.

## Next

- Optional: revisit the `[ax-execution]` "Bind new steps with _ax_execution"
  wording (pre-existing follow-up from 2026-10-06 log) — descriptive
  protocol text, no coding imperative, left unchanged.
