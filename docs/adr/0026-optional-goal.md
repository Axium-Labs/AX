# 0026. Optional Goal in the existing execution kernel

Status: accepted

## Decision

Evolve the former stop verifier in place into Goal. Retain one GoalEvaluator
boundary and the existing Agent Loop; never add a Goal dispatcher, database or
parallel validator. The task queue remains the sole identity/state checkpoint.
Ordinary turns omit Goal and retain direct streaming completion by default.

Global/project settings are templates for explicit target creation or opt-in
automatic Goal admission for ordinary new tasks. The host `enabled` preference
defaults false; explicit Goal lifecycle and supplied worker policies remain
independent. Shared CLI/TUI/ACP admission uses the existing Pursue intent and
preserves caller-owned task IDs. Legacy names
are converted at configuration/history boundaries. Acceptance is scoped to the
current target; worker verification is not inherited. Worker metrics come from existing child receipts; workers retain their own budgets
and do not inherit target completion policy. Round/time caps default to unlimited
and constrain existing ExecutionBudget/timeout paths. Token/billing caps require
complete reported statistics; estimates never enforce a hard cap. Harness progress
advances drive configurable stagnation, and existing RetryPolicy handles retries.

Configured Command, HTTP and host/MCP acceptance adapters wrap GoalEvaluator and
use the existing Scheduler, tool sandbox, permissions and Session evidence sink.
Rules, versions, bindings and unsafe in-flight effects persist with the Goal.
Conservative stagnation incorporates measured external/child evidence and parks
external waits for explicit host/user resume. Optional RuntimeExtension checkpoints
reuse Crew's fenced Workflow CAS for cumulative consumption and bounded receipts;
ordinary tasks without an optional Goal do not incur those calls. Local Session leases protect execution
and recovery writes. A missing verifier or unreconciled local effect prevents
automatic takeover; neither a new budget ledger nor a distributed scheduler is added.

Goals stop distinctly as completed, paused, blocked, failed or cancelled. Unknown
proof cannot become success. Persist conservative reservations before inference
and structured tool evidence before advancing; resume never automatically replays
unknown external effects. AXCrew retains distributed ownership and scheduling.

## Consequences

No extra reviewer request or token-estimation pass runs for ordinary prompts
unless the user enables the scoped Goal template.
Finite limits and evidence-based stagnation bound optional continuation. Hard
interruptions may overestimate display statistics and elapsed time; users inspect
recorded work and explicitly change limits/resume. External side effects and third-party tool
model costs cannot always be established by the kernel. See [goal.md](../goal.md).
