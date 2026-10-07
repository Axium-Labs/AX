# 2026-10-07 — Subagent refactor: adopt the deepseek-harness design (no switch)

Follow-up to `2026-10-07-harness-default.md`. User decision: refactor AX's
subagent after `deepseek-harness`'s design — delegation is a mounted
capability, not a user-toggled feature. Reference research: one implementation,
two tool instances (spawn = fresh context, fork = inherits completed turns);
config only tunes the concurrency pool and depth budget (defaults 8 / 1).

## Changed

- `crates/core/src/subagent.rs`:
  - `SubagentConfig`: removed `enabled`; `max_concurrent` default 3 → 8
    (deepseek `maxActiveSubagents`); `max_depth` default 1 unchanged, and `0`
    is now the only way to disable delegation.
  - `SubagentTool` became one implementation with a `fork` flag, registered
    twice by `prepare_subagents` (like deepseek's `tool-subagent` plugin
    instantiated as `subagent` + `subagent_fork`):
    - `subagent` — fresh-context delegate, deepseek's spawn wording; schema
      unchanged (task/agent/context/tools/policy).
    - `subagent_fork` (new) — `{description?, prompt}` only; fixed policy
      `context: completed_turns`, `workspace: shared`, parent model, no
      narrowing. Foreground: the call returns the child's final result.
  - `prepare_subagents` gates only on `max_depth == 0` / child kernels.
  - `run_child` injects the deepseek delegation contract into every child:
    permission scope fixed at start, denied operations must not be retried,
    limitations belong in the reply.
- `crates/core/src/child_policy.rs`: new `ContextInheritance::CompletedTurns` —
  seed = parent messages up to (excluding) the last user message, so the fork
  child never sees the in-flight turn that is delegating it.
- `crates/cli/src/capabilities.rs`: agent templates load whenever
  `max_depth > 0` (was `enabled && max_depth > 0`).
- CLI surface: `ax settings` dropped `--subagent true|false`
  (`crates/cli/src/args.rs`, `app.rs`); TUI `/settings subagent on|off` slash
  commands and the `enabled` toggle removed (`tui/commands.rs`), concurrency
  cycles 1–8. Legacy configs with a `subagent.enabled` key parse fine (unknown
  keys ignored) and legacy `config.toml` migration keeps
  max_concurrent/max_depth.
- Tests: fixture lost its `enabled` param; the
  `disabled_has_no_tool_*`/`toggle_off_*` tests replaced by
  `zero_depth_disables_delegation_without_initialization` and
  `reconfiguration_keeps_delegation_available_on_the_next_turn` (both tools
  registered by default).

## Verification

- `cargo check --workspace --all-targets`: PASS
- `cargo test --workspace`: PASS (30 binaries, 615 tests)
- `cargo clippy --workspace --all-targets -- -D warnings`: PASS
- Real binary (isolated AX_HOME, DeepSeek): asked the model to delegate
  "17*23" through the `subagent` tool — child spawned with no configuration,
  completed in an isolated child-run workspace, result 391 relayed; completion
  `direct`, 2 model steps, 1 tool.

## Docs

- `docs/tools.md` "Optional subagents" → "Subagents": always available, two
  tools, tuning-only config, fork contract, child delegation contract.
- `docs/capabilities.md`: named-Agent and project-settings wording aligned
  (no `enabled`).
- `docs/agent-runtime.md` trigger table unchanged (it describes model choice,
  not availability).

## Next

- `subagent_fork`'s shared workspace and inherited history make it the
  KV-cache-friendly path once a provider adapter exploits prefix caching; no
  adapter change is required for correctness.
