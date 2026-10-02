# 0016. Parallel child dispatch, structured receipts and resolvable instructions

Status: accepted; extends 0005, 0008, 0011, 0014, 0015

## Context

AX could already delegate work to isolated children, but the controller promoted
one task per model round: the queue's ready set existed and was never used. A
child reported `{success, output}`, so the controller had to re-read a child
transcript to learn what had happened, and re-running a child was the only way to
recover detail. There was no deterministic way to load project instructions, and
no way for the model to ask a question that materially changes the result —
its only options were to guess or to end the turn and hope the next user message
reopened the work.

Four goals drove this change: task success rate, total completion time, tool and
model call efficiency, and stability on long tasks.

## Decision

**Instructions are resolved, not recalled.** A deterministic resolver reads the
global instruction file, `AGENTS.md` at the repository root, `AGENTS.md` at every
directory from the root down to the cwd (`AGENTS.override.md` replaces the same
level), and `.ax/rules/*.md` whose `path`/glob scope matches a file the task
names. Every segment carries `source_path`, scope, priority and provenance, and
the most specific level wins. Nothing goes through memory, embeddings or a
semantic index, and project instructions are their own context slot: they are not
memory, not a skill and not system context.

**The runtime admits the whole ready frontier.** `pending` becomes `ready` when
every dependency is `Completed`. All ready, non-conflicting tasks are dispatched
to the existing `AgentSupervisor` in one batch, bounded by the supervisor's
concurrency, and newly ready tasks are admitted as others finish — the controller
never waits for one child before starting the next. Conflict is decided by the
existing resource model: a task may declare `resources` (path or name, with a
write flag) and two tasks whose declared accesses overlap are never concurrent.
A failed or timed-out child records its receipt and the frontier moves on.

**Children return a receipt, not a sentence.** `ChildResult` carries id, task id,
status, summary, findings, changed files, diff statistics, diagnostics,
validation, artifacts, failure reason, a continuation hint and metrics (wall
time, model rounds, tool calls, reported tokens, failed tool calls). The model
only ever sees `model_summary()`; the full record is persisted, and the
`child_result` control tool reads it back by id (full, diagnostics, artifacts,
diff, validation, metrics) instead of re-running anything.

**Asking the user suspends the run.** `request_user_input` is a first-class
structured control tool for "what do you want here?" — never for "may I run
this?". Asking checkpoints the goal, queue, session and question, parks the queue
in `waiting_for_user`, and returns; the answer is written back to the same tool
call id and the run resumes from that position with no new user turn.

**Boundaries are unchanged.** The kernel still owns permission, sandbox,
resource conflict, dependency, timeout/budget and cancellation; the model still
decides what to search, read, delegate, change and try next. No progress gate,
recovery lock, fixed search order or planning invariant was added. Main and child
share one `ToolRegistry`; a child differs only by context, cwd, memory scope,
sandbox boundary and task input.

**The turn timeout is an idle timeout.** A turn that delegates legitimately spans
the whole child batch, and each child has its own budget, so the controller is
cancelled only when nothing has progressed for the whole window. Progress is a
model round, a tool round, a child dispatch or a child receipt.

## Consequences

Completion time improves because independent work overlaps instead of being
serialized behind one task per model round, and the controller's context stays
small because receipts are projected. Stability improves because a failure is
contained to its task, a resumed session never re-runs a completed child, and a
suspension is not a failure.

The costs accepted:

- Concurrent children provision workspaces at the same time, so disk and
  `SQLite` contention grows with concurrency. Child dispatch, provisioning and
  cleanup therefore share one admission lock per child root, and quota
  accounting tolerates a directory that is being retired instead of failing an
  unrelated child.
- The controller's turn timeout no longer bounds a delegated batch by wall
  clock. A batch is bounded by the per-child budgets instead, which is where the
  timeout is meaningful.
- `task_queue` gained a `resources` field and `GoalTurn` gained an `Answer`
  variant; both are additive and old checkpoints still load.
- Subagent and child hosts implement `ChildCheckpoint::finish(&mut ChildResult)`
  instead of receiving a `ChildOutcome`, so a host can enrich the receipt with
  facts only it knows (for example an authoritative `git` diff).
