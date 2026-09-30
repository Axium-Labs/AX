# 0006. Skill evolution outside the runtime kernel

Status: accepted

## Context

Long-lived AX usage contains recurring workflows and corrections, but learning
must not turn every request into another analysis call or grant an agent
permission to rewrite its kernel. Standard Skills and scoped Memory already
provide the appropriate behavior/persistence seams.

## Decision

Observe existing runtime events in the CLI, and hand bounded Experiences to
an in-process worker. Analyze on session boundaries or accumulated batches,
with a persisted cooldown and timeout. Reuse bundled skill-creator instructions
and existing package creation/validation. Keep lifecycle/ownership metadata in
a private ledger, separate from standard SKILL.md. Only unchanged, project-owned,
`source=evolved` packages are eligible for automatic mutation. Trial/active
packages join ordinary discovery after built-in/user sources; candidates and
archives remain outside routing. Composite evidence and utility determine
learning and retirement, with merge/compression ahead of creation.

## Consequences

The core loop, tool implementations and permission policies stay independent
and unchanged. Ordinary requests perform bounded event collection and queue
handoff, with no evolution model call. Local raw learning history is retained.
Learning is initially project scoped, model-dependent and best effort across
abrupt exit/queue overload. Standard exports do not yet carry the private
evolution ledger. Manual edits relinquish automatic mutation of a package.
