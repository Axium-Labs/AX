# 0017. Authoritative Experience JSONL and Evolution control checkpoints

Status: accepted

## Context

Evolution duplicated full observations in its bounded JSON ledger and append-only
JSONL. Its recent-window deduplication could append old IDs again, and the ledger
could not recover a JSONL append whose control checkpoint had failed.

## Decision

Complete Experience records belong only to `experiences.jsonl`. A version-2
`ledger.json` contains Skill lifecycle metadata, epoch, scheduling counters and
separate byte cursors for observed usage and successful analysis. Recent evidence
is a bounded runtime buffer. The worker retains its Engine; an in-memory ID/offset
index is streamed once at initialization and extended only from appended bytes.
Analysis selects a contiguous pending prefix and cannot commit excluded records.

Legacy migration checkpoints original boundaries before appending missing IDs,
then publishes the control-only ledger. JSONL wins conflicts. Existing action,
Skill format, Memory and project-scope contracts remain unchanged. There is no
new database or crate: sessions/experiences/decisions use JSONL, Memory/queryable
indexes use SQLite, Evolution control uses JSON, and Skills use `SKILL.md`.

## Consequences

Failed analysis or persistence retains the pending cursor. Restart reconstructs
recent evidence and deduplication from the authoritative stream. The ephemeral
ID/offset index grows with record count, and initialization scans history once.
Multi-store actions remain at-least-once across crashes; conservative migration
can reanalyze missing previously consumed copies. Torn or corrupted raw records
fail closed without destructive repair. Older AX writers cannot share a migrated
store. Derived correction annotations from old ledger copies do not replace raw
JSONL; Skill correction counters remain intact.
