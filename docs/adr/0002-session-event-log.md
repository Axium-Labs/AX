# 0002. SQLite index + JSONL session event log

Status: accepted

## Context

AX persists raw session history that must survive crashes, compression and
long-running sessions. Storing full message bodies inline in SQLite bloats the
database and makes backup, audit and truncation awkward. Storing everything in
plain JSONL alone makes metadata queries and pagination slow. Compression
needs a watermark that says "what was summarized" without deleting anything.

## Decision

- Use a **hybrid layout**: SQLite holds session/message metadata, indexes and
  facts; each session's complete messages live in an append-only JSONL event
  stream (`sessions/<uuid>.jsonl`).
- **JSONL is the source of truth; SQLite is a rebuildable index.** Message
  rows point into the JSONL via `event_offset`/`event_length`; inline content
  is cleared after the event is written.
- **Write order is JSONL first, SQLite after** (inside an `IMMEDIATE`
  transaction): the event can never be lost, and the index can always be
  rebuilt.
- `sync_session_locked` runs before every read/write: it detects missing or
  stale index entries, rebuilds them from JSONL, and migrates legacy inline
  rows — making crash recovery automatic and idempotent.
- Compression writes a per-session summary + `through_message_id` watermark
  and never deletes raw events.

## Consequences

- ~1.1–1.2× disk overhead versus pure SQLite, in exchange for a complete audit
  trail and crash-safe writes.
- Old and new versions must not run against the same data directory: legacy
  versions ignore JSONL and can create inconsistencies.
- Manual edits to JSONL break exact-offset indexing and are unsupported;
  backup means copying both the SQLite file and the JSONL files.
- Migration from the legacy pure-SQLite layout is automatic and transparent —
  see [storage.md](../storage.md).
