# 0008. Durable lightweight Task Queue

Status: accepted

## Context

Tool-round DAG isolation does not prevent an execution model from ending a turn
after one failed subtask. Context compaction and reconnect must preserve remaining
work without introducing a separate planner or replacing the scheduler.

## Decision

Keep ordered task metadata in the runtime kernel and checkpoint it through the
existing AgentState/JSONL path. Only an explicit model `task_queue start` creates
a queue; top-level numbered/bullet formatting never creates executable tasks.
Expose a small queue tool for model-selected semantic task boundaries.
Only the compact current progress state enters each execution request. Task-local
failures permit advisory recovery and advance independent work. Keep existing
execution budgets global. Gate final content output and TurnFinished on terminal
queue state plus the final summary, with cancellation and global errors allowed
to stop execution.

## Lifecycle correction

Session identity is storage ownership, not goal identity. Every new top-level
user goal gets a fresh goal ID and supersedes/archives resumable prior work.
Resume is an explicit operation matching that ID. Task completion is an explicit
queue transition, never inferred from a final text response. Global blockers,
fatal errors and cancellation stop queue consumption and cache one terminal
response; budgets suspend work. Task-local failures continue independent tasks.
Worker forks isolate queue metadata while recording parent goal identity.

## Consequences

No new planner, scheduler, database schema, dependency or startup work. Latest
queue state is restored separately from bounded conversation context and survives
provider changes. Semantic decomposition and dependency-based skipping still use
the execution model's judgement. Recovery never blindly replays tool side effects.

## Child execution extension

A queue can attach a composition-root `ChildHost` to provision independent child
sessions, workspaces and memory stores. The controller dispatches entries through
`AgentSupervisor::run_child`, reusing the existing kernel loop rather than a new
executor. Tools explicitly bind child scope instead of sharing parent session
state. Child terminal receipts precede controller advancement for reconnect
recovery. Local child failures advance independent work; global controller stops
remain terminal/suspended as before. No large planner or fixed prompt is added.

Local child persistent state now lives outside the disposable workspace. Git inputs
are HEAD plus a binary dirty patch and filtered untracked files; non-Git inputs use
filtered snapshots. Terminal receipts allow immediate workspace removal without
breaking controller recovery. Leased running/interrupted workspaces alone remain
resumable; bounded admission/checkpoint quotas and lazy expiry GC limit their disk
usage while preserving raw history. GC never relies solely on an old heartbeat to
delete a workspace that is leased by a running process.

See [0015](0015-shared-child-composition.md) for shared frontend wiring, complete task inputs and replacement of unexecuted plans.
