# 2026-10-04 — Continuation-driven completion

Task: Remove mandatory Completion Review and make ordinary turns complete directly.

Changed:
- core continuation.rs, loop_runtime, kernel, child_dispatch/subagent: one structured Continue/Wait/Complete projection from runtime facts; tool/result/task/child/input/retry state drives execution, without a complexity classifier.
- core stop_guard.rs: optional StopGuard trait, default absent; deterministic deliverable/successful-call checks and explicitly enabled model verification. Controller guard requirements are not inherited by worker forks.
- CLI config/child_runtime: global verification object and project .ax/verification.json override, shared by CLI/TUI/run/ACP and AX Crew's ACP bridge.
- Model text streams immediately, including coding harness and active queues. Default final adds no model request. Separate continuation/completion/guard events include goal identity, execution steps and guard request counts; AX_EVENT_LOG uses the shared prompt boundary.
- Child recovery retains read-only compatibility with old completion-check records; new guard-pending markers are only written for explicitly enabled guards, in the final's checkpoint. Provider-declared continuation cannot recover as terminal.
- Added test/harness/continuation.rs and subagent_continuation.rs; existing frontend/workspace mocks now reject default completion-review requests. Updated current architecture/harness/ACP docs and ADR 0018. Historical 0.3.4 acceptance evidence is explicitly labeled historical.

Why:
- Ordinary answers should require one execution model request and zero reviewer requests. Past tool, edit, shell, build/test, task or subagent use must not add a final audit round.

Validation:
- 15 new continuation tests PASS: Q&A/code explanation one request, immediate streaming, chained tool results, pending task/subagent/result/approval/retry/user input/steer, deterministic guard, explicit model guard and denial, child recovery.
- cargo test --workspace: 550 passed, 0 failed, 3 ignored. Includes CLI/TUI/ACP real local-child and workspace integration coverage.
- cargo clippy --workspace --all-targets: exit 0; existing tool/CLI fixture and app warnings remain, no new core warnings.
- cargo fmt --all --check and git diff --check PASS.
- AX website content checked: no completion-review claims or affected release versions to update. AXCrew code unchanged; it inherits the kernel behavior through ACP.

Limitations / Next:
- Built-in verification modes are off/deterministic/model. Agent/command/policy guards can implement StopGuard; no built-in agent verifier is claimed.
- Deterministic deliverable checks establish existence, not freshness or semantic correctness. Required successful calls use explicit IDs and structured tool outcomes.
- No live paid-provider benchmark, release, commit or installation performed. Existing untracked work logs preserved.
- No required implementation work remains for this refactor.
