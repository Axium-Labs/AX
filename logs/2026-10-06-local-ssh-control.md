## 2026-10-06 14:33

Task: Local AX controls remote hosts through SSH without starting remote AX

Changed:
- crates/tool/src/ssh.rs: configured multi-host ssh list/exec tool; fixed local OpenSSH
  invocation, quoted remote cwd and stdin scripts, timeout and host validation.
- runtime/builder.rs: SSH contexts keep inference, credentials and transcripts local;
  register SSH/web tools rather than local filesystem/shell/MCP tools.
- Tool trait/core scheduler: independent SSH hosts bypass the bounded local execution
  pool, retaining permissions, dependency ordering and per-host resource conflicts.
- execution.rs: forward a file-backed SSH context into WSL and translate key paths.
- memory migrations: immediate writer transactions prevent concurrent-open snapshot races.
- session_projects.rs: locked, atomic registry updates prevent concurrent transcript loss.
- test/ssh.rs and test/concurrent_memory_open.rs; tools/storage/architecture/README docs.

Why:
- Correct the prior remote-AX design to the user's explicit local-controller requirement.
- No fixed host count or cross-host SSH concurrency cap; large host catalogues use a
  manifest file rather than exceeding Windows environment block limits.

Tests:
- cargo fmt, cargo clippy --workspace --all-targets: PASS (existing warnings remain).
- Final cargo test --workspace: PASS; CLI 155 passed / 2 benchmarks ignored.
- SSH catalogue regression: 1,024 hosts; validation/resource tests: PASS.
- Concurrent shared SQLite open/write regression (12 threads): PASS.
- Real local AX/Crew process regression with mock OpenSSH: PASS, 264 stored hosts,
  six hosts used in one resumed session, eight simultaneous same-member tasks,
  no AX or model credentials on remote shell and local history available offline.
- An earlier broad test run failed one existing child-runtime assertion; focused and
  final whole-workspace reruns both passed. Cause not established.

Limits:
- External SSH authentication/server and WSL runtime not live-tested.
- Requires local OpenSSH and remote POSIX shell. Host authentication uses existing
  SSH keys/agent/config; interactive password/passphrase prompts are unsupported.
- No fixed SSH count cap; actual capacity depends on OS, network and model resources.

No version change, commit, release or installed binary replacement.
Preserved existing distributed-collaboration edits and historical logs.
