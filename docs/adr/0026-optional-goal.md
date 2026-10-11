# 0026. Optional Goal in the existing execution kernel

Status: accepted

## Decision

Evolve the former stop verifier in place into Goal. Retain one GoalEvaluator
boundary and the existing Agent Loop; never add a Goal dispatcher, database or
parallel validator. The task queue remains the sole identity/state checkpoint.
Ordinary turns omit Goal and retain direct streaming completion.

Global/project settings are templates for explicit target creation. Legacy names
are converted at configuration/history boundaries. Acceptance is scoped to the
current target; worker verification is not inherited. Worker metrics come from existing child receipts; workers retain their own budgets
and do not inherit target completion policy. Round/time caps default to unlimited
and constrain existing ExecutionBudget/timeout paths. Token/billing caps require
complete reported statistics; estimates never enforce a hard cap. Harness progress
advances drive configurable stagnation, and existing RetryPolicy handles retries.

Goals stop distinctly as completed, paused, blocked, failed or cancelled. Unknown
proof cannot become success. Persist conservative reservations before inference
and structured tool evidence before advancing; resume never automatically replays
unknown external effects. AXCrew retains distributed ownership and scheduling.

## Consequences

No extra reviewer request or token-estimation pass runs for ordinary prompts.
Finite limits and evidence-based stagnation bound optional continuation. Hard
interruptions may overestimate display statistics and elapsed time; users inspect
recorded work and explicitly change limits/resume. External side effects and third-party tool
model costs cannot always be established by the kernel. See [goal.md](../goal.md).
