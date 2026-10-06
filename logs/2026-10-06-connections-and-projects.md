## 2026-10-06 00:28

Task: AX workspace discovery for AXCrew connections/projects

Changed:
- crates/cli/src/acp_workspace.rs and ACP dispatch: read-only _ax/workspace with canonical cwd,
  parent and directory-only entries; missing/file paths rejected.
- Strict sandbox confines canonical cwd, listed symlink destinations and parent to workspace.
- _ax/capabilities advertises workspace method.
- crew_device heartbeat capabilities now include registered AX project roots.
- test/acp_workspace.rs, README and ACP/architecture docs updated.

Why:
- Let AXCrew inspect actual local/SSH directories and select paired-host registered roots
  without weakening existing bridge execution allowlists or ACP session cwd matching.

Tests:
- cargo fmt --check, cargo check --workspace: PASS.
- cargo clippy --workspace --all-targets: PASS with existing warnings.
- cargo test --workspace: PASS (CLI 155 passed / 2 benchmark tests ignored, other crates passed).
- AXCrew real process connection regression: PASS for host roots, invalid path rejection,
  remote multi-turn/history and immutable model environments.
- Existing Crew smoke/model/gateway/workspace recovery regressions: PASS.

Issues:
- AX and AXCrew must both use this source build for the new workspace contract.
- External POSIX SSH host not live-tested; paired AX execution tested locally with real binaries.

No version change, commit, release or installed binary replacement.
Preserved preexisting distributed host/worker changes and historical logs.
