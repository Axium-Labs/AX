## 2026-10-03 13:06 (Asia/Shanghai)

Task: Make experiences.jsonl authoritative and ledger.json control-only.

Changed:
- crates/evolution/src/{engine,storage,types,worker,lib}.rs
- crates/cli/src/evolution.rs (existing cancellation regression assertions only)
- Moved crates/evolution/src/tests.rs to test/evolution/lifecycle.rs
- Added test/evolution/persistence.rs
- docs/evolution.md, docs/storage.md
- docs/adr/0017-evolution-jsonl-authority.md, docs/adr/README.md

Summary:
- Removed full Experience copies from the serialized ledger; Experience model unchanged.
- Version 2 retains lifecycle metadata, epoch, pending count, cooldown and separate observed/processed byte cursors.
- JSONL append is synced before telemetry checkpointing; runtime-only recent evidence is bounded.
- Worker caches Engine and extends its memory ID/offset index from appended bytes; initialization streams JSONL once.
- Analysis consumes bounded pending prefixes; context fitting cannot advance past omitted pending evidence.
- File/audit/Memory failures retain the analysis cursor; failed telemetry/cursor checkpoints preserve runtime progress.
- Legacy migration deduplicates by ID, preserves raw JSONL and Skill counters, recovers orphan appends, and checkpoints original migration offsets before missing-record appends.
- CREATE/REFINE/MERGE/PROMOTE/RETIRE/MEMORY/IGNORE, scope, lifecycle guards, decisions format and SKILL.md format retained.

Tests:
- cargo test -p evolution: PASS (29 tests)
- cargo test --workspace: PASS (518 passed, 3 existing ignored)
- cargo clippy -p evolution --all-targets -- -D warnings: PASS
- cargo clippy --workspace --all-targets: PASS; existing CLI/tool warnings remain, no Evolution warnings
- cargo fmt -p evolution -p cli: PASS
- git diff --check: PASS

Issues / compatibility:
- Older writers expect ledger.experiences and must not write a migrated store.
- If legacy copies disagree with JSONL, raw JSONL wins; derived correction annotations are runtime-only, Skill correction counters survive.
- A missing previously consumed legacy record appended after pending work can be conservatively analyzed again.
- ID/offset memory grows with unique records; startup rebuild scans history once, normal consumption uses seeks.
- Multi-store action writes are not one transaction. Analysis can retry after an action was committed but before its cursor checkpoint.
- Torn, malformed, truncated streams and invalid/future control schemas fail closed; no raw user data is overwritten to repair them.

Version / commit follow-up:
- Bumped AX from 0.3.2 to 0.3.3 in Cargo.toml and all AX-owned package entries in Cargo.lock.
- Added docs/release-notes/0.3.3.md; cargo check --workspace: PASS.
- Windows x64 release build: PASS; release executable smoke check: PASS.
- Local archive `release/v0.3.3/ax-x86_64-pc-windows-msvc.zip` created and SHA256 verified.
- Release notes and local release index prepared. Tag push will trigger GitHub release and configured GitCode mirror.
- Source/version commit: 68b859d8fdef8fcebb7241aabbdfb3ce68618374; release-note follow-up commit pending.
