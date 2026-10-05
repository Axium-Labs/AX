# 0021. Durable distributed collaboration at the CLI boundary

Status: accepted

## Context

Multiple independent AX instances must cooperate across Hosts without replacing
the existing loop, local tasks, sessions, memory, subagents or Crew device bridge.
The AX that originally plans a workflow may disappear while remote work continues.

## Decision

Add an outbound pull worker and an opt-in Tool at the CLI composition boundary.
Use existing ACP execution in isolated per-attempt workspaces. AXCrew is the
single authority for durable Tasks, Events, Artifacts, Workflow State, placement,
leases and retries. Scheduling matches AX capabilities and shared Host resources.
Fence all writes with incarnation/generation/lease identities. Reuse stable
parent-scoped delegation IDs across owner replacement. Save selected checkpoints
and results instead of replicating full model Context, Sessions or Memory.

## Consequences

Single-machine startup and runtime-core remain unchanged. Workflow recovery does
not require a permanent Coordinator Agent or high-frequency peer communication.
Local model/provider/tool configuration stays independent. Recovery starts a new
ACP attempt from durable inputs and checkpoints; it does not restore every local
execution detail. At-least-once work needs idempotent external effects. SQLite
provides a single control-plane authority; replicated consensus and object storage
are future deployment extensions, not current guarantees.
