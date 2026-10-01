# 0008. Durable lightweight Task Queue

Status: accepted

## Context

Tool-round DAG isolation does not prevent an execution model from ending a turn
after one failed subtask. Context compaction and reconnect must preserve remaining
work without introducing a separate planner or replacing the scheduler.

## Decision

Keep ordered task metadata in the runtime kernel and checkpoint it through the
existing AgentState/JSONL path. Recognize explicit top-level lists directly;
expose a small queue tool to the execution model for semantic task boundaries.
Only the compact current progress state enters each execution request. Task-local
failures get a recovery opportunity, then advance independent work. Keep existing
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
