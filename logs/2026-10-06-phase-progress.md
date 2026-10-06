## 2026-10-06 15:33

Task: Reduce per-tool narration for related work

Changed:
- crates/cli/src/runtime/tool_policy.md: communicate in the user's language,
  report a related tool sequence as one phase, update only on useful findings,
  obstacles or approach changes, and finish with one coherent answer.
- docs/tools.md: document the policy shared by CLI/TUI and ACP/Crew sessions.

Why:
- User requested fewer repetitive intermediate replies in AX and AXCrew.
- Crew presentation now groups/folds tools and limits answer actions to the final
  response; kernel events and model prose remain preserved for history.

Tests:
- cargo check -p cli: PASS.
- git diff --check: PASS.
- Related Crew transcript/UI/browser regressions passed (see AXCrew log).

Limits:
- This is runtime model guidance; it cannot guarantee a particular provider's
  narration frequency. No changes to tool scheduling or persisted transcripts.

No version change, release, commit or installed binary replacement.
