# AX Goal

Goal is the in-place evolution of the former StopGuard. There is one completion
validator and one existing model/tool Agent Loop. It is optional and off for
ordinary tasks. Harness still owns reliable execution, Task Queue owns task
identity and children, and Goal owns acceptance and continuation limits.

## Use

- `/goal <target>` starts and executes a natural-language target (model mode when
  the template is off). Include acceptance in the target, or supply explicit
  `acceptance` strings in the JSON form below.
- `/goal` or `/goal status` shows the target, state, last decision and consumption.
- `/goal pause` preserves unfinished work. `/goal resume` explicitly resumes the
  same ID. Answer a parked user question before resuming.
- `/goal cancel` or `/goal off` cancels the optional target. Ordinary next prompts
  always start normal tasks; they never inherit the previous Goal's criteria.
- `/goal create {"target":"produce report","mode":"deterministic","deliverables":["report.json"],"max_rounds":20}`
  sets per-target criteria and limits. Relative deliverables resolve against the
  project root (including paths supplied in this JSON command).
- `/goal limits {"max_rounds":64,"max_tokens":200000}` changes limits on a paused
  or blocked target. Consumed resources remain cumulative; raising a limit does
  not reset usage or evidence. Explicit resume follows.

These commands work in TUI and through `ax run "/goal ..."`. Reopen the saved
session with `--session` to inspect/manage a target after process exit. Active TUI
pause/cancel interrupts the existing turn handle and records interrupted tool
results; status remains available while streaming. Other text continues to steer
normal work. ACP uses the same prompt boundary and accepts the existing
`_meta.axGoal` GoalTurn (`pursue` with `config` and optional caller-owned `goal_id`,
`resume`, `pause`, `cancel`, `answer`). A native `/goal` prompt preserves an existing
caller `Start` identity instead of creating a competing Goal ID.
Optional Goal state/usage updates are supplied in `session/update` metadata.

## Configuration and migration

Global `$AX_HOME/config.json` uses `goal`; a project may override the template in
`.ax/goal.json`. Templates configure only explicit Goal creation, never ordinary
conversations or automatic worker validation.

```json
{
  "mode": "model",
  "acceptance": ["report agrees with actual measured results"],
  "deliverables": ["report.json"],
  "required_successful_calls": [],
  "max_rounds": 0,
  "max_seconds": 0,
  "max_tokens": 0,
  "max_cost_microusd": 0,
  "input_microusd_per_million": null,
  "output_microusd_per_million": null,
  "max_stagnant_checks": 3
}
```

Global `verification` is migrated atomically to `goal` on config read, preserving
unknown user fields; an existing `goal` wins. TOML compatibility uses the same
configuration alias and writes the new JSON name. On explicit Goal creation,
legacy `.ax/verification.json` is converted atomically to `.ax/goal.json` when the
new file is absent. The original project file remains as a backup. Core only
receives GoalConfig; it has no legacy configuration branch. Existing durable
pending/allowed markers remain readable for old child recovery.

## Completion and limits

Deterministic mode checks declared deliverable existence and successful **call
IDs** from structured results belonging to this target. It never interprets
assistant claims or shell prose as success. Existence does not prove freshness
or semantic correctness. An empty deterministic check set, or natural-language
acceptance in deterministic mode, yields insufficient evidence instead of success.

Model mode first checks configured deterministic requirements, then makes an
independent streaming provider request. Its only schema is `goal_decision`:
allow, continue, blocked, failed, insufficient_evidence with a concrete reason.
It receives the current target, acceptance, execution evidence and child receipts.
It cannot execute tools; its text is never a second final response. Continue and
insufficient evidence feed a reason back to the existing loop. Blocked requires
external action and explicit resume; failed and cancelled are terminal. Typed
permission denial blocks Goal without granting any additional permission.

Round/time limits default to **unlimited** (zero). A Goal may configure them without
changing ordinary tasks. Runtime constrains the existing `ExecutionBudget.max_steps`
by remaining cumulative Goal rounds and uses the existing timeout path for the
remaining active runtime. One execution or verification request chain is a round;
bounded retries remain part of that step. Existing tool, child and execution budgets
continue to apply. Child execution retains its own existing budgets and scheduler;
Goal does not add a distributed/global worker budget. Completed child metrics are
absorbed once from current queue receipts for display.

Reported usage comes from existing `ModelResponse.usage` and child metrics. Token
limits are enforced only when every counted request has complete token reports;
missing reports, retries, interruption, semantic compression or delegated usage make
the aggregate conservative/estimated and disable its hard token cap. Cost limits
require complete provider-reported `cost_microusd` billing. Explicit micro-USD rates
per million tokens can produce display estimates, but **never** hard admission.
Absent billing/rates displays unavailable. A cost limit without rates is valid.
No invoice price, cached-token discount or arbitrary tool-internal model cost is
inferred. Reported caps stop subsequent work when exhausted; an in-flight request
can cross a cap, since AX does not know its final usage before the response.

Stagnation reuses Harness `ExecutionState.progress.advances` and declared deliverable
metadata. The default is 3 unchanged completed loop rounds after the first observation;
`max_stagnant_checks: 0` disables it. Repeated assistant claims, tool failures or
unchanged observations do not establish progress. Resource exhaustion and stagnation
pause without completion. Provider errors use the existing finite RetryPolicy:
exhausted transient execution errors pause, permission/configuration faults block,
and unrecoverable errors fail. Verifier failure never silently allows completion.
Users can inspect evidence, change limits and explicitly resume.

## Persistence and recovery

The existing `[ax-task-queue]` checkpoint has one optional `goal` object: target,
criteria, state, consumption, validation result, proof IDs, Harness progress
baseline and pending estimates. Its enclosing queue supplies the same `goal_id`/`parent_goal_id` used
by Harness, Task, Session and AXCrew. Planning cannot replace this object. No new
database, dispatcher, memory store or distributed protocol is added.

Inference estimates and tool evidence are checkpointed through the existing
JSONL-first sink before advancing. Interrupted states restore as paused, without
automatic execution. Unknown in-flight usage is displayed conservatively as an
estimate, without a hard Token/cost cap. Interrupted wall time through recovery is counted conservatively;
completed pauses exclude time.
Completed child receipts are reused. Pending tool calls get an unknown-side-effect
placeholder and a recovery instruction to inspect actual external state before
retrying. AX cannot independently prove all external effects, so no blind replay
is performed. A durable allow marker recovers a verified final without another
model request. Accepted steering or pending work must still be consumed before
completion. Streaming precedes validation; only successful admission emits
Completion and TurnFinished.

Embedders use `GoalTurn::Pursue`, `configure_goal` (next target only), `goal()` and
`GoalEvaluator`/`with_goal_evaluator`. Custom evaluator implementations must be
reinstalled explicitly after restoring a session; they are executable host policy,
not serializable configuration. AXCrew retains distributed scheduling, ownership,
failure detection and takeover responsibilities.

## Validation

Scripts live in `test/`; run with isolated AX_HOME/TEMP and outputs under the
workspace `axtest/`. `test/harness/goal.rs`, `test/goal_commands.rs`,
`test/goal_tui.rs`, `test/goal_process.py` and existing continuation/child suites
cover direct completion,
verification, lifecycle, budgets, proof scoping, persistence, tool permissions and
frontend wiring. `test/goal_process.py` runs the production binary against a local
SSE provider and kills/restarts it after a persisted filesystem effect.
Use `test/goal.ps1 -Workspace -Process` for the reproducible run; it restores the
calling shell's environment variables afterwards.
