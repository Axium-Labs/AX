# 0009. Goal-bound execution invariants

Status: amended by [0014](0014-advisory-execution-policy.md)

The predecessor-progress and recovery admission decisions below are historical.
ADR 0014 retains resource/safety constraints and authoritative execution history,
but replaces semantic execution gates with advisory orchestration policy.

## Context

Conversation summaries preserved stated goals but did not bind actual tool calls
to steps, directories or observed progress. Recovery encouraged workspace discovery
without an enforced boundary. Compressed history could make the model misreport
its own tool usage, especially when the UI displayed child tools.

## Decision

Keep a lightweight serializable ExecutionState in the existing kernel. The current
execution model supplies step bindings on ordinary calls; the runtime checks goal
identity, predecessor progress and declared resources. Actual completion events
update state and are checkpointed through AgentState; restoration bypasses bounded
conversation selection. Child completion events are absorbed with durable cursors.

An eight-call no-progress window triggers bounded step recovery and contextual
reselection, with no keyword routing or separate planner. Three failed retries
force a strategy change. Search has explicit workspace fallback and fixed traversal,
depth, file-size and elapsed-time ceilings. Tool-owned storage uses an explicit
resource declaration rather than tool-name permission heuristics.

## Consequences

Scope and progress are enforceable independently of conversation compression.
Raw history stays authoritative, and short tasks add no model round trips.
Read-heavy tasks can receive a conservative stagnation intervention after eight
observations; structured result evidence can verify outputs. Successful declared
mutations are evidence of execution, not a semantic proof that an arbitrary result
satisfies the user. Extensions with opaque effects cannot launch new recovery
operations without scoped resource declarations. State projections consume the
existing ContextBudget and event previews are bounded.
