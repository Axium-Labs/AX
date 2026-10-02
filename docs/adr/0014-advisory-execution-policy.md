# 0014. Advisory execution policy and tool liveness

Status: accepted; amends 0009

## Context

A successful empty catalog could not advance a step without positive progress.
The rejection entered recovery, which then rejected opaque shell/MCP tools.
Retry limits and observation budgets could eliminate every useful next action.
These checks confused orchestration decisions with execution safety.

## Decision

Keep authoritative ExecutionState history and progress measurements. Completed
calls transition running to observed regardless of positive progress. Retry,
replan, next step and explicit failed task outcomes remain available. No-progress
and recovery context advise the model without gating tool admission. Recovery
records the failed step, call ID, tool and resources; it never narrows scope.
Legacy recovery snapshots restore the original declared scope before admission.

Keep permission, sandbox, explicit resource scope, conflict leases, dependencies,
configured budgets/timeouts and cancellation enforcement. Resource::All still
requires an exclusive lease; it is not a reason to forbid recovery diagnostics.
Search fallback notes are advisory and cannot authorize scope escape. Failed
tasks may finish without a mandatory recovery call; dependency failures still
skip their dependents and global/fatal blockers still stop a goal.

## Consequences

Empty/no-match observations and nonfatal failures leave retries and diagnostics
available under the same safety policy. Independent subtasks can continue.
Orchestration can suggest strategy changes but cannot globally lock tools.
Semantic success remains distinct from observation and safety authorization.
