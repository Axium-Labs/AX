# Context, Compression & Resume

This document covers how AX budgets the model's context window, compresses it
when it grows too large, and restores sessions across runs. The storage that
backs these flows is documented in [storage.md](storage.md).

## Overview

"Context" means the messages actually fed to the model for one turn. AX never
deletes raw history; it manages how much of it is *visible* to the model, and
compresses the visible portion when pressure builds.

## ContextBudget

`ContextBudget` (`runtime-core::budget`) is the single source of every
context-space limit. Instead of each module hard-coding character counts or
fixed fractions of the raw window, all limits derive from one structure:

```text
context window (provider-reported)
  − RESERVED_OUTPUT_TOKENS           → room for the reply
  − tool schema token estimate       → room for the tools actually sent
  ────────────────────────────────
  usable()
  − SKILLS_RESERVE_TOKENS            → skill instructions
  − MEMORY_RESERVE_TOKENS            → retrieved memory
  ────────────────────────────────
  history_budget()
  ─ SESSION_SUMMARY_SHARE_PERCENT    → persisted summary / agent state
  ────────────────────────────────
  recent_messages_budget()           → verbatim recent conversation
```

Key behaviors:

- `compact_threshold()` is a percentage of `usable()`, not of the raw window.
- The summary share is capped so a large summary can never crowd out the
  recent messages.
- Every layer is an explicitly named method/constant; adding a new consumer of
  context space starts from this structure rather than a new magic number.

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
`compact_threshold()` derived from `ContextBudget`.

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

- **`/resume`** — opens a previous session (up to 50 listed). Selecting one
  restores the saved effective context when available, then adds messages
  after its compression watermark. Older database versions fall back to the
  session summary plus uncompacted messages. `select_context` bounds the
  restored model context using the active model's budget, while the transcript
  UI separately restores the complete stored history. The selected session
  becomes current, the runtime is rebuilt on demand, and session-scoped
  permission decisions reset.
- **`/new`** — starts a fresh session. The active session reference, loaded
  messages, active skills and runtime are cleared and session permissions are
  reset. The SQLite session row is created lazily on the next prompt, using
  that prompt for its title. The previous session stays stored and can be
  reopened with `/resume`.

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
