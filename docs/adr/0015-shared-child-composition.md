# 0015. Shared child composition and explicit complete task inputs

Status: accepted; extends 0008

## Context

ACP command dispatch precedes interactive setup, making frontend wiring parity
hard to audit. The current ACP adapter already uses the common prompt runner,
but lacked tests proving actual child execution through that adapter. List-derived
legacy plans and a blanket nonempty-queue replan ban can confuse benchmark rules
with work and prevent explicit replanning before any task executes.

## Decision

Keep one Agent Loop and a shared prompt composition boundary. CLI, TUI and ACP
bind LocalChildHost, project scope and both execution budgets through one helper
and constructor. Test each prompt adapter with a real LocalChildHost, not a fake
host or forced prompt instruction.

Only explicit task_queue start creates executable tasks. Accept full task-input
strings for compatibility and title/input objects for distinct display titles.
Children receive complete input, and controller progress includes it. Dynamic
instance enumeration happens after an ordinary data read and an explicit start.

An active same-goal queue with no dispatched tasks, receipts or outcomes may be
replaced; archive its previous state without deleting raw history. Checkpoint
execution_started before dispatch/provisioning to conservatively preserve unknown
in-flight side effects. Executed and terminal work requires an explicit new goal
for supersession. Old string checkpoints use their stored title as full input.

## Consequences

Crew gets the same isolated child sessions, sequential queue consumption,
independent child budget and failure continuation as CLI/TUI. No extra planner,
Agent Loop, permission bypass or list heuristics are introduced. Runtime validates
input presence; semantic completeness remains the model/caller's responsibility.
