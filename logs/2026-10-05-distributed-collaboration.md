# 2026-10-05 — Optional Distributed Collaboration

Task: Add asynchronous cross-Host collaboration over durable Tasks, Events,
Artifacts and Workflow State while preserving the existing AX runtime.

Changed:
- CLI `crew worker`, `distributed_client`, `distributed_tool`, `distributed_worker`.
- Optional `collaboration` Tool in the runtime builder, isolated per-attempt Git
  clones/non-Git snapshots, scoped credentials, bounded result reports/artifacts.
- Existing ACP prompt metadata additively exposes `waiting_for_user` for unattended
  execution; the agent loop, local Subagents/Tasks/Sessions/Memory stay unchanged.
- Device bridge accepts legacy Crew paths only for canonical registered project
  roots; arbitrary/nested paths remain denied. This repairs the existing Crew
  transport's cwd contract without weakening workspace confinement.
- `test/distributed_worker.rs`, `test/crew_compatibility.rs`, docs/README maps,
  ACP/architecture/development docs and ADR 0021.

Summary:
- AXCrew owns assignment, leases, retry/cancel/failover and capacity reservations.
  AX owns reasoning and existing ACP execution.
- Stable parent-scoped request IDs, workflow checkpoints and known task results
  allow another AX to resume planning without a permanently live Coordinator.
- Logical project identity maps to local paths. Input Patch artifacts are verified
  and applied only to isolated execution workspaces; sources stay untouched.
- Windows Git arguments normalize verbatim canonical path prefixes. Large failure
  output uses artifact references so report-size rejection cannot strand a task.

Tests:
- `cargo test --workspace`: PASS (CLI 151 passed, 2 benchmarks ignored; remaining
  workspace crate tests passed), including 4 distributed worker regressions and
  registered-root legacy compatibility.
- `cargo clippy --workspace --all-targets`: PASS; existing unrelated warnings remain.
- `cargo fmt --check`, debug AX build: PASS.
- AXCrew `tests/integration/distributed_process.py`: PASS — three real AX processes on two
  logical Hosts, failed test → analysis → Patch → retest, Crew restart, scoped auth.
- Existing Crew smoke process suite: PASS — local/remote DAG, permission approval,
  cancel/retry, disconnect/resume and pairing/revocation.

Issues / limits:
- Processes simulate Hosts on one Windows computer with a fake model and explicitly
  disabled sandbox in the distributed integration fixture. Physical networking,
  GPU assignment and production OS enforcement are not verified by this scenario.
- Execution is at least once; external mutations need idempotency. Worker crash
  restarts durable work from inputs/checkpoints, not full local model sessions.
- Current Crew is one SQLite authority, artifact limit 8 MiB, no automatic GC.

Next:
- Deployment-level physical multi-Host/TLS/network partition verification; resource
  enforcement and scalable artifact/state storage if required by deployment.
- No version bump, commit, publication or installed-binary replacement performed.
