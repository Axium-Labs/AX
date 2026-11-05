# Reviewing and maintaining contributions

**English** | [简体中文](reviewing.zh-CN.md)

[Contribution entry point](../../CONTRIBUTING.md) · [Navigation](README.md)

## Roles and authority

- **Contributor:** proposes a focused change, obtains design agreement where
  required, supplies appropriate tests and docs, reports validation honestly and
  responds to review. Opening a PR grants no merge authority.
- **Reviewer:** any community member may inspect and test a PR, identify risks
  and suggest improvements. Explain findings with evidence; distinguish required
  correctness/safety changes from optional suggestions. Community approval is
  feedback, not permission to merge.
- **Maintainer:** sets scope and design direction, resolves conflicting feedback,
  assesses validation and has final authority to accept, reject, close or merge
  a PR. Repository write permissions remain controlled by maintainers.

There are no additional contributor levels or mandatory review committees.

## Review criteria

Use current [Architecture](../architecture.md), [Development](../development.md),
topic documents and accepted [ADRs](../adr/README.md) as technical standards.
Apply these questions to the changed behavior; explain inapplicable areas.

- **Architecture and crate boundaries:** Does the change belong in the crate
  owning the responsibility? Does it preserve dependency direction, CLI
  composition and the kernel's separation from task-specific behavior?
  Are significant deviations discussed and recorded through the ADR process?
- **Runtime and Agent Loop:** Are model/tool/event ordering, context budgets,
  continuation, termination, errors and resume consistent with the
  [runtime](../agent-runtime.md) and [loop](../agent-loop.md) contracts? Check
  cancellation, deadlines, cleanup, child isolation and concurrency where
  affected; look for deadlocks, unbounded retries and duplicate execution.
- **Memory and Storage:** Do [memory](../memory.md) scope/identity boundaries and
  [storage](../storage.md) semantics hold? Check additive migrations, existing
  data compatibility, raw JSONL history, recovery and partial-write handling.
  Tests should use isolated data and show that state is neither lost nor mixed
  across projects/sessions.
- **Permissions, security and Sandbox:** Verify declared tool permissions,
  explicit denial/approval handling, independent [host access](../host-permissions.md),
  confinement, subprocess/MCP boundaries and secrets. Compare enforcement with
  [security](../security.md) and platform limits; action approval must not silently
  bypass OS confinement. Review adversarial and denied paths as well as success.
- **Performance and resources:** Does lazy initialization remain intact? For
  changed hot/startup paths, request reproducible before/after measurements with
  workload and environment. Assess bounded memory, concurrency, I/O, latency,
  process lifetime and cleanup rather than unsupported speed claims.
- **Tests and regression risk:** Do tests cover the failure and changed behavior,
  including relevant errors and platforms? Is the final public file set
  independently buildable? Are private results distinguished from tracked
  workspace coverage and compilation-only checks?
- **Public APIs, providers, MCP and dependencies:** Review exported Rust types,
  CLI/config/event and protocol compatibility, errors and migration needs.
  Check [provider](../providers.md) and [MCP](../mcp.md) negotiation, streaming,
  cancellation and authentication where affected. Justify new dependencies,
  license/MSRV/platform implications and intentional lockfile changes; avoid
  unnecessary dependencies and unrelated upgrades.
- **Documentation:** Can users and developers understand the resulting behavior,
  limits and compatibility? Are topic docs, examples, links and relevant ADRs
  updated without duplicating the development standard?

## Constructive review

Review purpose and major risks before style details. Cite the affected path or
contract, describe the consequence and suggest how to verify the fix. Label
optional nits as nonblocking; keep required requests actionable. Ask when
evidence is missing. Discuss disagreements respectfully; maintainers settle
unresolved design and scope questions.

After updates, review changed areas and verify previous concerns are addressed.
Approval of an earlier revision does not establish correctness of new behavior.
For stalled work, ask about the contributor's plans before a takeover and preserve
attribution; see [coordination](issues.md#claim-and-coordinate-work).

## Merge gate

Before merging, a maintainer confirms:

1. Scope is appropriate; required issue design agreement exists and significant
   decisions are represented in the ADR process.
2. Substantive correctness, security, compatibility and architecture concerns are
   resolved, with maintainer review of the final revision. CI does not replace it.
3. Required PR checks pass on the final revision and applicable platform and
   integration evidence is adequate. Environment limitations are documented;
   the maintainer decides what additional evidence is needed before merge.
   Limitations do not justify hiding a failing check.
4. Tests and docs match behavior, dependency/API changes are explained and the
   committed file set works without private resources.
5. The PR is mergeable, focused and contains no credentials, local logs, private
   test artifacts or unrelated changes.

These are review policies. Adding a workflow does not configure GitHub branch
protection or required-check settings; maintainers manage those settings
separately. Maintainers retain the final merge decision and explain rejection
or closure. This guide does not add release permissions.
