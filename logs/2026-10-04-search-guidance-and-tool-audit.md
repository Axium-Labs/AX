# 2026-10-04 — Search recovery guidance and tool instruction audit

Task: Soften automatic shell-search fallback guidance as authorized; inspect similar rigid guidance in AX and report it without changing other behavior.

Changed:
- crates/tool/src/web.rs and web/router.rs: failed searches recommend reporting errors/configuration fixes/retry; discourage automatic bypass of providers. Explicit user-requested alternative search and network diagnostics are allowed subject to existing tool permissions. Fetch stays the known-URL retrieval operation.
- skills/web-research/SKILL.md, docs/tools.md and ADR 0019: same explicit user-request exception. Historical logs preserved.
- test/web/search_router.rs: revised failure-guidance regression checks the default restriction, explicit user exception and permission caveat on both error and tool description.

Audit (read-only; no edits to these parts):
- shell.rs:250-255 + find.rs:12: shell description says Fallback only and prohibits recursive scans when discovery can express them; find says shell only when no discovery tool can express the request. Execution validates empty commands and Windows 5.1 syntax, without a discovery-vs-shell gate. Absolute wording may discourage user-requested native shell commands or specialized diagnostics.
- search.rs:31: forbids using search to inspect located files and retrying identical no-match queries. These are prompt instructions; scheduler.rs:126-151 only reuses identical read-only discovery calls within one round. No cross-round no-match ban. Targeted regex/usage analysis and searching after file changes may require these operations.
- patch.rs:38: Read only the affected lines first. Implementation validates hunks/stale context atomically; it does not enforce a narrow preread. Broader function/call-site context may be needed before editing.
- filesystem.rs:43/50: Use patch for existing file edits; schema calls write a new-file operation. Actual write at 121-125 uses tokio::fs::write and can overwrite an existing file, subject to permission handling. Guidance lacks explicit alternatives for requested full-file replacement/generated artifacts.
- skills/code-review/SKILL.md:19-22: limits reporting to introduced regressions and excludes pre-existing issues. Appropriate for a diff review, but an explicitly requested whole-project audit may need a wider scope. Skill guidance only, no runtime report filter.
- Legitimate constraints distinguished from routing preferences: shell.rs:258 Windows 5.1 syntax validation; permissions/sandbox boundaries; task_queue/request_user_input/child_result singleton handling in loop_runtime/tool_step.rs. These are real runtime/platform constraints.
- execution.rs:412 already describes diagnosis/replanning with other tools as advisory and keeps tools available after empty results/no progress. It does not hard-block shell fallback.

Validation:
- cargo test -p tool --test search_router --test web: 45 passed (18 routing + 27 web).
- cargo test --workspace: 567 passed / 0 failed / 3 ignored.
- cargo clippy -p tool --all-targets and cargo clippy --workspace --all-targets: PASS; existing warnings remain, no warnings introduced by this update.
- cargo fmt --all --check and git diff --check: PASS. Superseded categorical web-search instruction strings removed from current source/skill/docs.

Next:
- Other audited instructions await user judgment; no unrequested changes made.
- No commit, release, installation or live paid-provider call performed.
