# 2026-10-04 — Default tool preferences with task/user exceptions

Task: Apply all five instruction audit findings authorized by the user.

Changed:
- crates/tool/src/shell.rs + find.rs: prefer dedicated tools for routine discovery; allow shell scans when explicitly requested, dedicated tools are unavailable, or native filters/pipelines are needed. Shell is directly described as appropriate for builds, tests, Git and CLI workflows. Removed blanket Fallback only/discovery exclusivity wording.
- crates/tool/src/search.rs: permit targeted regex/symbol/usage searches in known files. Avoid unchanged repeated searches without new evidence; changed files, revised patterns/scopes or explicit user verification justify another search. Empty results remain successful observations.
- crates/tool/src/patch.rs: read sufficient relevant context before editing, including surrounding functions/callers when needed, instead of only affected lines.
- crates/tool/src/filesystem.rs: prefer patch for localized edits; describe write accurately as creating or replacing an entire file, including requested whole-file rewrites/generated artifacts. Description/schema clarify complete content replaces rather than appends. Inspect existing content before replacement; range/full-file reads follow context needs.
- skills/code-review/SKILL.md: metadata/body cover diff reviews and whole-project audits. Diff reviews default to newly introduced regressions; user-requested broader/pre-existing findings are allowed and distinguished. Whole-project audits cover existing issues; requested style/convention reviews use supplied standards.
- docs/tools.md + docs/skills.md: same preferences, exceptions, overwrite semantics and review scope. Updated the existing tool description routing regression in crates/tool/src/lib.rs.

Scope:
- Changes are model-facing descriptions, schema documentation, bundled skill instructions and docs; no executable tool-selection bans added or removed.
- Existing permission checks, sandbox boundaries, Windows 5.1 syntax validation, patch atomicity and within-round discovery result reuse retained.
- Preserved all pre-existing workspace changes and historical logs. AXCrew/website code untouched; this guidance change has no affected website claims.

Validation:
- cargo test --workspace: 567 passed / 0 failed / 3 ignored.
- cargo clippy --workspace --all-targets: PASS; existing warnings remain, no new warnings introduced by this update.
- cargo fmt --all --check and git diff --check: PASS.
- Verified superseded blanket tool-selection/read-context guidance no longer exists in current tool source and relevant skill/docs.

Next:
- All five authorized findings addressed. No required implementation work remains.
- No commit, release or installation performed. Existing installed copies of bundled skills are not changed by editing the repository.
