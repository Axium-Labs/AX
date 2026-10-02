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
| Token-aware selection | `crates/core/src/context.rs` |
| Compaction, kernel loop | `crates/core/src/lib.rs` (`AgentKernel::compact_now`, `compress`) |
| Session snapshot storage | `crates/memory/src/lib.rs` (`save_effective_context`) |
| Dispatch and pickers | `crates/cli/src/tui/commands.rs` (`execute_slash`, `open_session_picker`) |
| Session restoration | `crates/cli/src/main.rs` (`ReplState::open_session`, `reset_new_session`) |
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
advance execution; a text-only answer with unfinished tasks blocks the goal and
stops further model calls. Terminal response text is cached in durable state;
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

`--child-timeout-secs` limits each isolated child independently; it defaults to
unlimited and never implicitly ends the controller. `--turn-timeout-secs` remains
the global turn deadline. These options also apply to ACP child dispatch.


## Execution state and truthful tool history

The kernel checkpoints `[ax-execution-state]` after every tool completion, using
existing AgentState/JSONL storage. It keeps at most sixteen completion events
(call ID, tool, step, actual resolved arguments, result preview and progress),
plus a cumulative tool-call count. Full raw results remain in authoritative
history. The CLI restores the latest checkpoint independently of effective-context
snapshots and bounded history pages. Compression changes conversation only and
cannot replace execution state with a model-generated guess.

Each execution request receives a replaceable `[ax-execution]` projection with
the goal, step, expected output, scopes, recovery and actual events. Normal requests
show two recent events; new user turns and stalls show up to eight, fitted against
`ContextBudget`. This also supplies history/progress questions without matching
language-specific keywords. Missing older events mean unknown details, never zero
calls. Legacy/compressed sessions without a checkpoint expose an unknown total
and only backfill retained actual call/result pairs. Child completion events use the same session-prefixed IDs as UI events;
controller cursors prevent duplicate counting after reconnect. Child state remains
independent of controller state.
