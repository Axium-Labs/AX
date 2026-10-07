# 2026-10-06 — Neutral Agent Runtime (remove default coding harness)

Task: Refactor AX's default Agent Runtime behaviour to a neutral runtime:
the current user request defines the task; context supports it; tools are
capabilities; autonomy escalates only when necessary. Reference: local
DeepSeek Harness design (system-prompt sections, goal activation, repeat-tool
reminder) — ideas only, no TypeScript port.

## Changed

- `crates/core/src/runtime_core.rs` (new): neutral `[ax-agent-runtime]` and
  `[ax-delegation]` prompt sections. No repo/coding imperative.
- `crates/core/src/harness.rs`: `prepare_environment` now injects the neutral
  runtime prompt + assembled `[ax-capability-guidance]` for every run; the
  environment snapshot and `[ax-coding-harness]` POLICY are installed only when
  `with_coding_harness()` was explicitly called. Added `capability_guidance()`.
- `crates/core/src/loop_hygiene.rs` (new): advisory consecutive-identical-call
  detector; reminders at 3/5/8, never blocks, no global tool-call cap.
- `crates/core/src/kernel/{state,builder}.rs`: new `repeat_calls` field.
- `crates/core/src/loop_runtime/{mod,tool_step}.rs`: reset the chain per turn;
  observe each executed call and inject `[ax-loop-hygiene]` reminders.
- `crates/core/src/task_queue.rs`, `subagent.rs`: tightened trigger wording
  ("use only when the request is genuinely multi-item"; "do not delegate work
  one agent can complete").
- `crates/core/src/instructions.rs`: project instructions are supporting
  context and never turn an unrelated request into a task.
- `crates/tool/src/lib.rs`: new `Tool::guidance()` (default `None`).
- `crates/tool/src/{filesystem,find,search,shell,patch,web,ssh,task_source,
  view_image,sandboxed}.rs`: capability-scoped guidance; sandboxed wrapper
  delegates it.
- `crates/cli/src/runtime/builder.rs`: dropped `.with_coding_harness()` and the
  global `tool_policy.md` prompt.
- `crates/cli/src/child_runtime.rs`: children no longer force the harness.
- Deleted `crates/cli/src/runtime/tool_policy.md`.
- `test/harness/runtime_neutrality.rs` (new): A–G acceptance behaviours.
- `crates/core/src/tests.rs`: compression test threshold accounts for the new
  runtime prompt baseline.
- Docs: new `docs/agent-runtime.md`; updated `README.md`, `architecture.md`,
  `agent-loop.md`, `coding-harness.md`, `context.md`, `tools.md`.

## Root cause of the old over-execution

1. `runtime/builder.rs` enabled the coding harness unconditionally for CLI,
   TUI, ACP and Crew, so every request ran as a coding task.
2. The harness `POLICY` was a global system instruction to "execute requested
   deliverables until completed … attempt installation, creation or fallback …
   a final answer requires every known item terminal and all requested durable
   reports written".
3. `kernel/turn.rs` created an **Active** `TaskQueue` for every new goal when
   the harness was on, including a plain question.
4. A monolithic `tool_policy.md` global prompt carried capability rules and
   "emit ALL known independent calls / run required full tests" imperatives.
5. No loop hygiene for repeated identical calls.

## Tests

- `cargo check --workspace --all-targets`: PASS
- `cargo test -p runtime-core`: PASS (incl. 8 new neutrality tests, 1 threshold updated)
- `cargo test -p tool`: PASS

## Issues

- The neutral prompt adds a small fixed token baseline to every request
  (~3 short sections); one compression test threshold was updated accordingly.

## Next

- Consider moving `[ax-execution]` step-binding wording to a purely descriptive
  form if it ever reads as an instruction to continue.
