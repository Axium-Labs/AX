## 2026-10-05 15:34 +08:00

Task: Automatic Host inventory and post-connect instance capability configuration

Changed:
- crates/cli/src/distributed_host.rs, distributed_worker.rs, main.rs
- test/distributed_host.rs
- README.md, docs/distributed-collaboration.md, docs/adr/0021-durable-distributed-collaboration.md

Summary:
- Opt-in worker probes native Host logical CPU count/name, physical RAM, GPU count/names,
  OS/architecture and hostname. Ten-second probe deadline, sixty-second background
  refresh; lease heartbeat remains independent and failed inventory delivery retries.
- Unknown GPU/RAM stay null rather than inventing zero physical devices. One detected
  Host capacity is shared by all its AX instances in Crew.
- Worker reports active local model/Skill/MCP/Tool/role/permission/environment metadata.
  Connected-instance pending settings are checked on startup; users merge generated
  settings locally and restart, keeping credentials and project paths on the Host.

Tests:
- cargo test --workspace: PASS (CLI 154 passed, 2 benchmark cases ignored; other crates passed).
- Three new inventory regressions: PASS, including real Windows CIM detection.
- cargo fmt --check, cargo clippy --workspace --all-targets: PASS; existing unrelated
  warnings remain. Final CLI clippy after warning cleanup: PASS, no inventory warnings.
- cargo build -p cli: PASS.
- Crew tests/integration/distributed_process.py: PASS — three actual AX workers, two
  logical Hosts, no manual enrollment resources/capabilities, detected hardware,
  staged capability configuration followed by local restart/report, remote failure,
  Patch/retest and Crew restart recovery.
- Crew smoke.py, model_catalog.py, gateway_socket.py, workspace_recovery.py: PASS.

Issues / limits:
- Native probe verified on Windows; Linux/macOS implementations were not executed here.
- Inventory is capacity, not utilization or OS/GPU resource enforcement; integrated GPUs
  are included. Capability selection is not a health check of every external service.
- Process scenario uses logical Hosts on one Windows machine and fake model/sandbox off.

Next:
- Source changes only; no new version bump, commit, release or installed-binary replacement.
