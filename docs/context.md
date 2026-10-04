# Context, Compression & Resume

This document covers how AX budgets the model's context window, compresses it
when it grows too large, and restores sessions across runs. The storage that
backs these flows is documented in [storage.md](storage.md).

## Overview

"Context" means the messages actually fed to the model for one turn. AX never
deletes raw history; it manages how much of it is *visible* to the model, and
compresses the visible portion when pressure builds.

## ContextBudget

AX uses one elastic input pool:

```text
provider context window
  - hard output reserve (model max_output_tokens; fallback 8000)
  - actual tool schema estimate
  = usable shared context pool
```

There are no Skill/Memory/Summary percentage partitions. Legacy budget
accessors return maxima against this same pool; they are not additive
allocations. `ContextDemand { demand, minimum, maximum }` expresses actual
request needs. `allocate()` honors every hard minimum, rejects impossible
minima, then distributes the remaining pool in caller priority order.
The latest complete user turn and runtime progress are hard minima; selected
skills and relevant memory compete before older optional history. Whole
conversation turns remain intact. Summary/system context consumes only space
left after selected recent conversation.

`context_pool` in `.ax/config.json` centrally defines operational maxima and
the next-request reserve:

```json
"context_pool": { "next_request_reserve": 1024,
                  "tool_result_maximum": 4096, "recent_raw_maximum": 4096,
                  "memory_maximum": 8192, "skill_metadata_maximum": 4096 }
```

Before a request, compaction projects the growth that request will actually
carry: the queue summary context appended after the pressure check, capped by
`tool_result_maximum`, plus the next-request reserve. It checks projected input
against `usable()`, rather than triggering from current percentage occupancy.
The hard pressure boundary is the usable pool. Manual compaction uses the same
pipeline. No percentage threshold or fixed-fraction helper decides automatic
compaction, and the cleanup tier derives its retained slice from
`tool_result_maximum` rather than a private character count. All arithmetic
saturates for tiny windows.

## Context selection

`context::select_context` picks the history to restore for a turn by **token
budget** rather than fixed message count, and guarantees it never cuts through
an unfinished tool-call round. It is used both when resuming a session and
when the runtime assembles each request.

## Compression

Compression affects only what is fed to the model. Raw messages remain in
SQLite/JSONL; only the latest cumulative summary is upserted per session and
the watermark advances.

**Layered pipeline** (shared by automatic and manual compaction):

1. Tool-output cleanup;
2. Deduplication;
3. Semantic compression when still needed.

**Automatic** pressure checks run before each model request, using
projected next-request size against the shared `ContextBudget` pool.

**Manual** — `/compact` requests immediate compaction of the active session's
effective context:

1. The command reports the current estimated context size and removes the
   injected retrieved-memory context from the runtime.
2. If a runtime exists, it calls `AgentKernel::compact_now`; otherwise no
   compression runs.
3. The kernel runs the same layered pipeline as automatic compaction.
4. On success the CLI stores the effective-context snapshot and session
   summary; original messages are retained.
5. The UI reports before/after token estimates, or says there was nothing to
   compact.

## Sessions & resume

- **`/resume`** — searches recent sessions across projects registered in
  `~/.ax/session-projects.json` (up to 100 listed). Ctrl+P filters by project;
  Ctrl+R renames, Ctrl+D deletes, and Ctrl+N starts a new session. Projects
  are registered when a session is created or `/resume` is opened. Selecting one
  switches the working directory and project-local storage to that project,
  restores the saved effective context when available, then adds messages
  after its compression watermark. Older database versions fall back to the
  session summary plus uncompacted messages. `select_context` bounds the
  restored model context using the active model's budget, while the transcript
  UI separately restores the complete stored history. The selected session
  becomes current, the runtime is rebuilt on demand, and session-scoped
  permission decisions reset.
  The TUI shows a loading state during the restore operation.
- **`/new`** — starts a fresh session. The active session reference, loaded
  messages, active skills and runtime are cleared and session permissions are
  reset. The SQLite session row is created lazily on the next prompt, using
  a short first-clause title from that prompt. The previous session stays stored and can be
  reopened with `/resume`.

Typing `@` in the composer searches project file names on demand. A selected
reference is resolved inside the project root and its UTF-8 text is included
in the same context reserve as skills. Oversized, non-text, and out-of-project
files are skipped. Referenced content is saved as session agent state so it
survives resume; raw user messages still keep the `@` token.

The TUI keeps the reader's position when new streamed output arrives above the
composer. A footer hint marks unseen output; End returns to the latest text.

Interrupted tool calls saved just before a crash are restored with an explicit
marker and are never replayed automatically — see [memory.md](memory.md).

## Related slash commands

| Command | Purpose |
|---|---|
| `/compact` | Trigger immediate compaction of the active session's effective context |
| `/resume` | Open a previous session |
| `/new` | Start a fresh session |
| `/status` | Show context usage, execution limits, budget and latency snapshot (read-only) |
| `/exit` | Exit the TUI |

## Reference

| Concern | Code |
|---|---|
| Budget derivation | `crates/core/src/budget.rs` |
| Token-aware selection | `crates/core/src/context.rs` (`select_context`, `request_context`) |
| Compaction pipeline | `crates/core/src/compression/` (`pipeline.rs`, `summary.rs`) |
| Agent loop, model and tool steps | `crates/core/src/loop_runtime/` (`mod.rs`, `model_step.rs`, `tool_step.rs`) |
| Session snapshot storage | `crates/memory/src/context.rs` (`save_effective_context`) |
| Dispatch and pickers | `crates/cli/src/tui/commands.rs` (`execute_slash`, `open_session_picker`) |
| Session restoration | `crates/cli/src/repl/state.rs` (`ReplState::open_session`, `reset_new_session`) |
| Session restore helpers | `crates/cli/src/session_restore.rs` |

## Long task progress

Multi-task turns keep a durable internal Task Queue. Before each execution model
request, the kernel replaces a compact `[ax-progress]` state containing
`overall_goal`, `current_task`, `completed_count`, `failed_count` and
`remaining_tasks` (pending task numbers). Completed task bodies and full queue
checkpoints are never injected as progress. The current task retains its needed
details; the original request supplies the pending task definitions. Progress
space and the queue tool schema are charged to the existing `ContextBudget`;
progress is reserved before optional history selection.

The final summary request receives terminal statuses and concise outcomes once,
after every task is completed, failed or skipped. Explicit task completion controls
advance execution; a text-only answer with unfinished tasks keeps the goal active and
continues execution. Terminal response text is cached in durable state;
resuming a blocked/completed/cancelled goal produces no duplicate output events.
Full queue snapshots and archived goals are removed from model context.
Worker contexts also remove the controller queue. Compaction uses the existing
pipeline.

Explicitly delegated queue children default to their task input plus small runtime
metadata (session, cwd, memory scope, platform and shell). Explicit subagent
`ChildPolicy` can select bounded parent context, skill and memory inheritance;
controller queue state and sibling history are excluded. See [tools.md](tools.md). A resumed child reloads only
its own raw history and raw result reader. Child queue tools are disabled so a
child cannot accidentally orchestrate the controller queue. Parent progress
remains compact; the summary includes child workspace/session/state references and
concise results, without full child tool histories.

`--child-timeout-secs` is a total model/tool execution deadline for each isolated
child, unaffected by continued activity; it defaults to
unlimited and never implicitly ends the controller. `--turn-timeout-secs` is an
*idle* deadline: a turn that delegates spans the whole child batch and each child
has its own budget, so the controller is cancelled only when nothing has
progressed — no model round, tool round, child dispatch or child receipt — for
the whole window. These options also apply to ACP child dispatch.

Each child's receipt is a structured `ChildResult`. The controller's model
context receives only the compact projection (status, root cause, relevant files,
validation, suggested next step); findings, diagnostics, changed files, diff
statistics and metrics stay durable and are read on demand with the `child_result`
control tool. The durable index is written once per dispatch rather than once per
child, so n children cost one snapshot instead of n growing ones.

## Project instructions

Project instructions are resolved once per turn and injected as their own
replacable `[ax-project-instructions]` system message. Resolution order, least
specific first: the global instruction file, `AGENTS.md` at the repository root,
`AGENTS.md` at every directory from the repository root down to the cwd
(`AGENTS.override.md` replaces `AGENTS.md` at the same level), then
`.ax/rules/*.md` whose `path`/glob scope matches a file the task names — `@`
references and path-like tokens that exist in the repository.

Every segment carries its `source_path`, scope, priority and provenance, the most
specific level wins, and the order is total (priority, then source path), so the
same tree and prompt always produce the same instruction set. The resolution
never consults memory, embeddings or a semantic index, and the token allowance
comes from `ContextBudget::instructions_budget_tokens()` (`ContextPoolPolicy::
instructions_maximum`, default 4096). The always-on chain is never dropped; when
the budget runs out, the least specific path-scoped rules are omitted and the
omission is reported.

Instructions are re-resolved on every turn rather than persisted, so they cannot
go stale in a resumed session, and they are never stored in raw history. They are
a separate concept from memory, skills and system context: none of the four
substitutes for another.

## Asking the user mid-run

`request_user_input` suspends the run instead of ending it. The kernel checkpoints
the goal, the task queue, the session and the question, parks the queue in
`waiting_for_user`, and leaves the pending tool call unanswered on purpose. The
frontends show the question; the answer is written back as the tool result of that
same call id and `GoalTurn::Answer` resumes the run from that position without
opening a new user turn. A reconnect still finds the goal waiting, and suspension
is never reported as failure. Authorization never travels this path: dangerous
operations are decided by the permission system.


## Execution state and truthful tool history

The kernel checkpoints `[ax-execution-state]` after every tool completion, using
existing AgentState/JSONL storage. It keeps at most sixteen completion events
(call ID, tool, step, actual resolved arguments, result preview and progress),
plus a cumulative tool-call count. Full raw results remain in authoritative
history. The CLI restores the latest checkpoint independently of effective-context
snapshots and bounded history pages. Compression changes conversation only and
cannot replace execution state with a model-generated guess.

Each execution request receives a replaceable `[ax-execution]` projection with
the goal, step, observation status, expected output, scopes, recovery and actual events.
Progress/recovery projections are advisory: empty results, no-match and failures
are observations and never remove tool access. Normal requests
show two recent events; new user turns and stalls show up to eight, fitted against
`ContextBudget`. This also supplies history/progress questions without matching
language-specific keywords. Missing older events mean unknown details, never zero
calls. Legacy/compressed sessions without a checkpoint expose an unknown total
and only backfill retained actual call/result pairs. Child completion events use the same session-prefixed IDs as UI events;
controller cursors prevent duplicate counting after reconnect. Child state remains
independent of controller state.
