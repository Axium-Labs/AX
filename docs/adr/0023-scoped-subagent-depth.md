# 0023. Scoped recursive subagent limits

## Decision

Allow non-negative `max_depth` values, preserving defaults of depth 1 and
concurrency 8. Depth 0 disables both delegation tools. AX owns validation,
global persistence and project overrides; Crew manages the same values through
`ax settings --scope global|project`, with explicit save and reset actions.

Each delegated child receives an immutable absolute depth, inherited limits,
an immediate-parent workspace host and a delegation tool whitelist. Tools are
rebound to the child rather than copied from the parent. Children at the cap,
or with empty/forbidden tool selections, cannot delegate. General workflow
children and arbitrary worker forks remain without a delegation host.

One turn-wide pool limits live descendants, including waiting ancestors, and
admits at most 64 total tasks. Root tasks retain queueing; nested work fails on
exhaustion instead of queueing behind its own ancestor. IDs are unique across
the tree and descendant lifecycle events propagate without child transcripts.
Cancellation cascades down the tree and closes durable receipts.

Delegating tools do not hold process-wide effect leases while awaiting a child;
the child's actual effect tools acquire those leases. This prevents recursive
shared-workspace deadlock while retaining effect serialization.

## Consequences

This supersedes ADR 0010's disabled default and prohibition of recursive
delegation. Limits apply on the next parent turn. A model chooses whether to
delegate; raising a limit does not itself start work or widen permissions.
