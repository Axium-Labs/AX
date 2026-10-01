# 0010. Optional model-selected subagents

Status: accepted

## Context

AX already has a checkpointed Agent Loop, bounded tool-round scheduler,
AgentSupervisor, isolated ChildHost, registry rebinding and an approval policy.
Model-selected delegation must reuse these mechanisms and preserve disabled
behavior and cold-start guarantees.

## Decision

Default delegation to disabled. On enabled turns only, register one `subagent`
tool that calls runtime spawn/wait primitives. Admission is bounded and execution
uses ChildHost plus AgentSupervisor::run_child. Inherit provider and approval
policy, seed only explicit task/context, narrow the rebound registry and remove
delegation from worker forks. Return a final result envelope and emit compact
lifecycle events. Existing config JSON/TOML migration and CLI/TUI settings own
persistence; reload before each turn. Cancellation stops the same loop and closes
the host receipt.

## Consequences

No extra planner call, forced task split, parallel runtime or child-history
injection is needed. Disabled turns construct no manager or delegation schemas.
Host workspace/session/artifact semantics remain authoritative: local child
workspaces are disposable and are not merged into the parent. Depth is limited
to one; bounded admission prevents unlimited spawn in a turn.
