# Issues and design discussion

**English** | [简体中文](issues.zh-CN.md)

[Contribution entry point](../../CONTRIBUTING.md) · [Navigation](README.md)

## Search and report

Search [issues](https://github.com/Axium-Labs/AX/issues), including closed reports,
and [PRs](https://github.com/Axium-Labs/AX/pulls). Add evidence to an existing
report when it describes the same problem. Read the relevant
[topic documentation](../README.md) to distinguish intended behavior and known
platform limits from a defect. If documentation leaves a usage question
unanswered, an issue with that context is welcome; no separate service is required.

Use the [bug form](https://github.com/Axium-Labs/AX/issues/new?template=bug_report.yml)
to provide AX version/commit, OS/architecture, Rust version for source builds,
provider/protocol and sandbox mode where relevant, minimal reproduction,
expected and actual results, and sanitized diagnostics. Prefer an isolated
workspace and synthetic data over a live session database. Do not publish API
keys, auth files, private prompts, memory contents or personal paths.
For a suspected exploitable vulnerability, use GitHub's private vulnerability
reporting option if available; otherwise ask for a private maintainer contact
without posting exploit details. [Runtime security documentation](../security.md)
describes implementation limits, not a promise of an established disclosure
service or response time.

Use the [feature form](https://github.com/Axium-Labs/AX/issues/new?template=feature_request.yml)
to explain the user problem, concrete workflow, alternatives and proposed scope.
State affected crates/contracts and compatibility, security or performance
implications you know. Incomplete implementation knowledge is fine.

## Design discussion

Before implementing a new feature, architecture adjustment or substantial
Runtime, Agent Loop, Memory, Storage, Permission, Sandbox, Provider or MCP change:

1. Open or join an issue describing the problem, scope, alternatives and design.
   Identify affected contracts, data formats, risks and a validation plan.
2. Discuss trade-offs with reviewers and maintainers. Link architecture documents
   and ADRs; propose an ADR for decisions with lasting consequences.
3. Wait for explicit maintainer agreement in the issue on the design and scope.
   Silence, assignment or community interest is not design approval.
4. Link that agreement in the PR. Return to the issue if implementation
   materially changes the agreed design.

Small documentation corrections, isolated bug fixes and regression tests can be
submitted directly. Subsystem location alone does not make a fix significant;
changes to its semantics, public contract or safety boundary do.

## Claim and coordinate work

Comment with your approach and scope before working on an existing issue.
Check whether someone is already implementing it and coordinate with them.
Maintainers may assign issues where permissions allow, but assignment is not a
prerequisite and does not replace required design agreement. Link your PR.

If you stop, leave an update so someone else can continue. Before taking over
stalled work, ask the contributor about their plans; a maintainer can clarify
ownership if there is no response. Preserve attribution when reusing work.
There is no guaranteed review or completion deadline.

## Triage and resolution

Anyone may reproduce a report, reduce a reproduction, compare versions or suggest
related documentation and issues. Explain conclusions with evidence and keep
disagreement about the change, not the person. Maintainers decide priority,
scope, assignment and closure. Reports may be closed as duplicate, intended
behavior, out of scope or insufficiently reproducible, with a reason and relevant
links. New evidence can support reopening.

A fix goes through the [PR workflow](pull-requests.md). Use `Fixes #123` only
when merge should resolve the issue, and `Refs #123` for partial work.
Verification on a supported platform is useful follow-up evidence.
