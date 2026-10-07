# 2026-10-07 — Context semantics fix + coding-harness opt-in provenance

Follow-up to `2026-10-06-agent-runtime-neutrality.md`.

Task: Correct two design points raised in review.

1. Context must be *available by default*, not hidden behind an explicit user
   request. The invariant is "context may exist → it does not define the task →
   it does not trigger an action → the model uses it when the request needs it",
   not "context never appears".

2. `with_coding_harness()` opt-in must originate from user intent, never from
   environment heuristics.

## Changed

- `crates/tool/src/environment.rs`: added `EnvironmentContext::light(cwd, root)`
  (cwd/workspace_root/sandbox/network/shell, no executable probes) sharing a
  private `base(...)` with `detect`. `executables` now
  `#[serde(skip_serializing_if = "BTreeMap::is_empty")]`.
- `crates/core/src/harness.rs::prepare_environment`: `[ax-environment]` is now
  injected on every run — `light` when neutral, `detect` when the harness is
  enabled. Only the advisory step scope and `[ax-coding-harness]` POLICY stay
  harness-gated. `with_coding_harness` documents the provenance rule; added
  `coding_harness_enabled()` read-only accessor.
- `crates/core/src/runtime_core.rs`: core prompt lists runtime environment
  information among supporting context.
- `crates/core/src/instructions.rs`: instruction framing aligned with the
  "may be relevant; use when applicable; never override the current user
  request" wording.
- `crates/cli/src/config.rs`: new explicit user switch `harness.enabled`
  (`HarnessConfig`, default off, omitted when disabled, `deny_unknown_fields`).
- `crates/cli/src/runtime/builder.rs`: `with_coding_harness()` is applied only
  when `config.harness.enabled`.
- `crates/cli/src/model_selection.rs`: config round-trip test updated.
- `test/harness/runtime_neutrality.rs`: A/B/C now assert context is present but
  no action is triggered (C asserts the workspace *location* is reported while
  its *contents* are not); new
  `no_environment_heuristic_enables_the_coding_harness`.
- Docs: `agent-runtime.md`, `coding-harness.md`, `context.md`, `architecture.md`.

## Verification

- `cargo check --workspace --all-targets`: PASS
- `cargo test -p runtime-core`: 179 passed / 0 failed
- CLI `config::harness_config_tests`, `config::subagent_tests`,
  `model_selection::tests`, `runtime::builder::tests`,
  `commands::prompt_execution_tests`: PASS

## Audit result

No production path auto-enables the coding harness. `discover_project_root`
reads `.git`/`Cargo.toml` only to locate the project identity (memory scope,
instruction root); it never changes the agent mode.
