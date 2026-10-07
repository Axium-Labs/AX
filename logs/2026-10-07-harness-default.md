# 2026-10-07 — Adopt the DeepSeek Harness philosophy: no mode switch, harness is the runtime

User decision: adopt `deepseek-harness`'s design philosophy — composition is the
mode, there is no settings switch. AX's coding execution harness is now
**unconditional**: the previous neutral-by-default design and its
`harness.enabled` opt-in switch were removed entirely. This reverses the
2026-10-06/07 neutrality work's default; the useful mechanisms (per-tool
guidance, loop hygiene, context boundary, evidenced global stops) are kept.

## Changed

- `crates/core/src/kernel/{state,builder}.rs`: removed the `coding_harness`
  flag. `AgentKernel::new` is the harness runtime; `fork_with_messages` no
  longer copies a mode.
- `crates/core/src/harness.rs`: removed `with_coding_harness()` /
  `with_neutral_runtime()` / `coding_harness_enabled()`. `prepare_environment`
  now always injects `[ax-agent-runtime]` + `[ax-capability-guidance]` +
  full `[ax-environment]` (`EnvironmentContext::detect`, executable probes) +
  `[ax-coding-harness]` POLICY. Removed the `[ax-delegation]` injection (its
  anti-queue wording contradicted the per-goal queue; queue discipline lives in
  POLICY and the task_queue tool description).
- `crates/core/src/kernel/turn.rs`: `begin_goal` creates an Active queue for
  every new goal (empty queues complete silently with a direct answer).
- `crates/core/src/loop_runtime/tool_step.rs`: the `task_queue block`
  evidence gate (`global_stop_evidenced`) is now unconditional — a real
  `ToolError::GlobalBlocked` is the only evidenced global stop.
- `crates/core/src/execution.rs`: removed `advisory_step_scope`; model-declared
  step subscopes are advisory by construction, the workspace boundary stays hard.
- `crates/core/src/runtime_core.rs`: removed `DELEGATION_GUIDANCE`; CORE_GUIDANCE
  reframed to harness identity ("executing the current request").
- `crates/cli/src/config.rs`: removed `HarnessConfig` and the `harness` config
  field. Old configs containing the key are ignored (AxConfig has no
  deny_unknown_fields) — verified with a legacy-config run.
- `crates/cli/src/runtime/builder.rs`: no config branch; kernel is harness.
- `crates/tool/src/environment.rs`: removed `EnvironmentContext::light`
  (neutral variant, no remaining callers).
- Tests: `test/harness/runtime_neutrality.rs` rewritten as the harness
  acceptance suite (A–G plus unconditional-context assertions; every goal has
  a queue, plain questions complete directly); `execution_tests` (advisory
  subscopes, hard workspace boundary), `task_queue_tests` (GlobalBlocked
  fixture; unevidenced block rejected; reconnect after runtime blocker),
  `tests.rs` checkpoint lengths, `child_deadline`/`retry` timeouts (the
  one-time executable probe shares the first-turn wall clock).
- Clippy: `cargo clippy --workspace --all-targets -- -D warnings` had never
  actually passed — the previous "PASS" was a pipeline artifact (`| tail` masked
  the exit code). Fixed the pre-existing pedantic lints: `tool/tests/resources.rs`
  (by-ref), `app.rs dispatch` / `e2e_tests` / `workspaces.rs` / long fns
  (targeted allows), `child_benchmark_tests.rs` (file-level cast allows,
  fixture allow).

## Docs

- `docs/agent-runtime.md` rewritten: AX is a coding execution harness, no mode
  switch; per-goal queue, evidenced global stops, advisory subscopes.
- `docs/coding-harness.md`, `docs/agent-loop.md`, `docs/architecture.md`,
  `docs/tools.md`, `docs/README.md`: removed neutral/opt-in claims and the
  `[ax-delegation]` references.
- `axium-site`: checked; it never described a runtime mode ("provider-neutral"
  is about model adapters) — no change needed.

## Verification

- `cargo check --workspace --all-targets`: PASS
- `cargo test --workspace`: PASS (30 binaries, 616 tests)
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS (verified
  without pipeline masking; HEAD baseline confirmed pre-existing failures)
- Real binary (isolated AX_HOME, DeepSeek deepseek-flash): "1+1等于多少" →
  direct answer "2", 1 model step, 0 tools, goal queue created and completed;
  legacy config with a `harness` key parses cleanly.

## Issues

- `EnvironmentContext::detect` probes up to 9 executables once per process on
  the first turn (up to ~2s each worst case). Acceptable (matches the harness
  design, cached process-wide), but two test timeouts were bumped for it.
- The neutral-runtime acceptance behaviours from the 2026-10-06/07 audit are no
  longer the product default; they survive only as harness-mode behaviours
  (direct answers for plain questions still hold, via the completed empty queue).

## Next

- If passive-question behaviour regresses in practice (harness policy pushing
  the model to over-queue simple asks), tune POLICY wording rather than
  reintroducing a mode switch.
