# AX Goal

Goal is the in-place evolution of the former StopGuard. There is one completion
validator and one existing model/tool Agent Loop. It is optional and automatic
use in ordinary tasks defaults off. Harness still owns reliable execution, Task
Queue owns task identity and children, Goal owns acceptance policy, and Runtime enforces execution
constraints using its existing budgets and usage statistics.

## Use

- `/goal <target>` starts and executes a natural-language target (model mode when
  the template is off). Include acceptance in the target, or supply explicit
  `acceptance` strings in the JSON form below.
- `/goal` or `/goal status` shows the target, state, last decision and consumption.
- `/goal pause` preserves unfinished work. `/goal resume` explicitly resumes the
  same ID. Answer a parked user question before resuming.
- `/goal cancel` or `/goal off` cancels the optional target. Ordinary next prompts
  use the current scoped template only when automatic Goal is enabled; they never
  inherit the previous Goal's criteria.
- `/goal create {"target":"produce report","mode":"deterministic","deliverables":["report.json"],"max_rounds":20}`
  sets per-target criteria and limits. Relative deliverables resolve against the
  project root (including paths supplied in this JSON command).
- `/goal limits {"max_rounds":64,"max_tokens":200000}` changes limits on a paused
  or blocked target. Consumed resources remain cumulative; raising a limit does
  not reset usage or evidence. Explicit resume follows.
- `/goal acknowledge <validator-id>` records explicit review of an interrupted
  unsafe verifier's external effects. It clears that replay barrier without
  running anything; inspect the saved call and real external state first.

These commands work in TUI and through `ax run "/goal ..."`. Reopen the saved
session with `--session` to inspect/manage a target after process exit. Active TUI
pause/cancel interrupts the existing turn handle and records interrupted tool
results; status remains available while streaming. Other text continues to steer
normal work. ACP uses the same prompt boundary and accepts the existing
`_meta.axGoal` GoalTurn (`pursue` with `config` and optional caller-owned `goal_id`,
`resume`, `pause`, `cancel`, `answer`). A native `/goal` prompt preserves an existing
caller `Start` identity instead of creating a competing Goal ID.
Optional Goal state/usage updates are supplied in `session/update` metadata.
A structured ACP `answer` retains its supplied goal/question identity and payload;
the shared prompt entry does not reinterpret its accompanying text against a
different parked question. The kernel validates it against the saved question.

## Configuration and migration

Global `$AX_HOME/config.json` uses `goal`; a project may override the template in
`.ax/goal.json`. Templates configure explicit Goal creation and optionally
ordinary new tasks.
`enabled` defaults to false for existing and new configurations. Enabling it in
AXCrew Settings → Goal automatically applies the resolved global/project template
to each new ordinary task in CLI, TUI and ACP; `mode: off` uses model verification
in this path. The prompt becomes the target. Changes apply on the next new task,
without altering saved Goals, explicit resume/pause/cancel, parked answers or
active steering. Disabling retains configured rules and explicit `/goal` use.
Project templates replace the whole global policy, including this switch; reset
inherits global settings. Ordinary admission does not migrate a legacy project
verification file. Controller-owned distributed workers and local subagents keep
their supplied policies rather than automatically adding acceptance from a host
template.

```json
{
  "enabled": false,
  "mode": "model",
  "acceptance": ["report agrees with actual measured results"],
  "deliverables": ["report.json"],
  "required_successful_calls": [],
  "validators": [],
  "validation_combination": "all",
  "max_rounds": 0,
  "max_seconds": 0,
  "max_tokens": 0,
  "max_cost_microusd": 0,
  "input_microusd_per_million": null,
  "output_microusd_per_million": null,
  "max_stagnant_checks": 3,
  "stagnant_seconds": 300,
  "max_unobserved_checks": 12,
  "stagnation_action": "pause"
}
```

Global `verification` is migrated atomically to `goal` on config read, preserving
unknown user fields; an existing `goal` wins. TOML compatibility uses the same
configuration alias and writes the new JSON name. On explicit Goal creation,
legacy `.ax/verification.json` is converted atomically to `.ax/goal.json` when the
new file is absent. The original project file remains as a backup. Core only
receives GoalConfig; it has no legacy configuration branch. Existing durable
pending/allowed markers remain readable for old child recovery.

`ax goal-settings --scope global|project` reads the same templates without opening
a provider or Session. `--json-stdin` replaces that scope with validated GoalConfig
JSON from stdin (1 MiB maximum); `--reset` restores global defaults or removes the
project override. Reset archives a legacy project template so it cannot be
silently migrated again. Other configuration fields are preserved. Project files
replace the global template as a whole; missing fields use GoalConfig defaults.
Portable relative references resolve in the selected project at Goal creation.
Settings edits apply to new tasks/Goals; saved Goals retain their frozen configuration.

## Configured validators

The existing GoalEvaluator is wrapped with configured acceptance conditions;
there is no separate loop or tool executor. A deterministic Goal can use custom
conditions without declaring files or successful call IDs. Model mode retains
its independent evaluation after custom conditions pass.

```json
{
  "mode": "deterministic",
  "validation_combination": "all",
  "validators": [
    {"id":"build","version":"v2","kind":"command",
     "command":"python scripts/verify.py","reference":"scripts/verify.py",
     "timeout_seconds":30,"replay_safe":false},
    {"id":"ci","version":"v1","kind":"http",
     "url":"https://ci.example.org/ax/acceptance","timeout_seconds":30},
    {"id":"enterprise","version":"v3","kind":"host","tool":"mcp",
     "input":{"action":"call","server":"enterprise","tool":"acceptance",
              "arguments":{}},"timeout_seconds":30,"replay_safe":false}
  ]
}
```

Command calls the registered `shell` tool, with a JSON request on stdin rather
than interpolating it into shell text. `reference` identifies an existing verifier
file; it is checked before execution and again after restore. HTTP is GET through
the existing `web.fetch` tool, with the JSON request in the `ax_goal` query
parameter; responses must be nontruncated `application/json`. It inherits Web
network policy, redirect checks and response limits. Host calls an existing
registered tool/MCP capability with an `ax_goal` argument. For the lazy `mcp`
gateway, context goes in `arguments.ax_goal` and `action` must be `call`.
Direct JSON receipts and standard MCP `structuredContent` or a single text block
are supported; MCP `isError` cannot admit completion.

All three use Scheduler permission admission, resource leases and sandboxed tool
registrations. Configuration cannot promote an unbound tool out of confinement,
inject scheduler control fields, or grant permissions. A 1–86400 second timeout
(default 30), bounded by the current Runtime tool timeout, covers approvals,
resource waits and execution. Raw tool results, call IDs and typed receipts are
checkpointed using the existing Session/Task Queue evidence path.

The request contains `protocol: "ax.goal.v1"`, `goal_id`, `target`, `acceptance`,
`validator_id` and `version`. A successful command writes exactly one JSON receipt
on stdout (logs may use stderr); HTTP and host return the same receipt:

```json
{
  "protocol":"ax.goal.v1", "goal_id":"<request goal_id>",
  "validator_id":"build", "version":"v2",
  "decision":{"status":"allow"},
  "evidence":{"exit_code":0,"artifact":"report.json"}
}
```

`allow` requires nonempty evidence. `continue` means acceptance is unfinished;
`insufficient_evidence` means it cannot currently be established. `blocked` needs
external action or permission, and `failed` is an explicit irrecoverable result.
The latter four include a nonempty `reason`. Transport errors, malformed or
truncated results yield insufficient evidence; identity/version mismatch and typed
permission denial block. They never silently pass. `all` requires every condition;
`any` requires one valid condition. Neither can hide blockers or irrecoverable
failures. Configured file/call requirements still apply after that combination.

The Goal persists the complete rules, declared versions/references, tool schemas,
command OS/architecture binding, receipts and in-flight verifier. Restore rechecks
availability/bindings; live service availability is established by the bounded
call, not assumed from saved success. Host GoalEvaluator implementations must
reinstall the same `policy_reference()` on the new host. Commands and host calls
default to unsafe replay; interrupting one retains a barrier until explicit
acknowledgment. Read-only HTTP GET is replay safe. A policy author may explicitly
mark an idempotent command/host verifier `replay_safe`.

## Completion and limits

Deterministic mode checks declared deliverable existence and successful **call
IDs** from structured results belonging to this target. It never interprets
assistant claims or shell prose as success. Existence does not prove freshness
or semantic correctness. An empty deterministic check set, or natural-language
acceptance in deterministic mode without a custom validator, yields insufficient
evidence instead of success.

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
Goal does not add a distributed/global worker budget ledger. Completed child metrics are
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

Stagnation combines Harness `ExecutionState.progress.advances`, real queue/child
transitions, deliverable metadata and fresh verifier work observations. A receipt
may include `work` with `status` (`running`, `waiting_external`, `no_progress`,
`blocked`, `unknown`), a concrete `reference`, optional measured `progress` and
`valid_for_seconds`. Observations expire (freshness is capped at the larger of
30 seconds and the configured idle interval). Unknown evidence is never failure.
Inline tools and child collection are awaited; approvals, running children and
retries do not count as stagnation. Observed background/external work parks the
Goal with its activity and evidence, without spending inference polling it.
The user/host resumes when actual work changes; Goal adds no background scheduler.

Defaults require 3 unchanged observations **and** 300 active idle seconds, with a
finite fallback at 12 unchanged observations. `max_unobserved_checks` is floored
at `max_stagnant_checks`; the idle interval can be zero for immediate legacy
thresholds. `max_stagnant_checks: 0` explicitly disables detection. `pause` is the
default action; `request_human` blocks for evidence review. Repeated assistant
claims, tool failures or unchanged reads do not establish progress. Resource exhaustion and stagnation
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
Single-machine CLI execution and Goal edits use an OS Session lease, released on
process death. Opening an owned Goal session cannot repair/write its history;
execution also rejects stale Goal/configuration/consumption snapshots. Loading
state does not start execution. Embedders retain responsibility for their own
Session ownership and choose whether to issue an explicit resume.
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

## AXCrew durability boundary

The existing distributed worker binds explicit `/goal` to the Durable Task ID.
It stores the queue, cumulative usage/reservations and bounded existing Message
evidence under `Workflow.state.ax_goal_runtime[task_id]`, using existing Workflow
CAS and source lease/incarnation/generation checks. It checkpoints before model
requests and effects, and before reporting completion. Missing authoritative
state on takeover stops execution rather than resetting consumption. Normal
distributed tasks do not write Goal checkpoints. Collaboration checkpoints
preserve the reserved Goal key. Crew's existing 256 KiB Workflow limit applies;
checkpoint failure stops advancement.

Interrupted running Goals may use Crew's existing takeover policy. Intentionally
paused/blocked Goals require explicit worker `resume_goals: true` (default false)
before an unattended retry. This does not clear unsafe effects, bypass validators
or reset limits. Goal pause/block remains in the Workflow; an unsuccessful worker
attempt uses Crew's existing failed-attempt reporting, not a new Goal scheduler.

Full local Session databases, child sessions and working-directory effects are
not migrated. Unfinished Goals with ordinary tool calls or local children stop
automatic takeover for host reconciliation of artifacts/effects. Policies with
paths outside the saved workspace are rejected; workspace-relative references
are rebased and verified on the new host. Available validators and unchanged
budgets are prerequisites, not assumptions of identical machines. Final result
submission remains fenced by Crew's existing task owner and generation.

Design reference: [Claude Code Goal](https://code.claude.com/docs/en/goal) and
[Hooks](https://code.claude.com/docs/en/hooks). AX keeps its own existing evaluator,
Runtime permissions/budgets and AXCrew ownership boundaries.

## Long Project and multi-Session execution

An explicit Goal can opt into `long_project` and its default automatic Session
rotation. Goal criteria, cumulative usage, runtime state and permission ceilings
survive rotation; working Markdown remains a non-authoritative evidence projection.
Native `/goal project` controls documents/configuration and `/goal rotate` requests
a safe handoff. Ordinary tasks remain unchanged unless rotation is explicitly
configured. See [Long Project and Session rotation](long-project.md) for defaults,
scoped overrides, import/export, conflicts and recovery.
