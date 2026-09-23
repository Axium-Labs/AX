# Architecture Decision Records

ADRs record design decisions with lasting consequences: the context, the
decision, and the trade-offs accepted. They make the *why* behind the code
discoverable.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001-memory-scopes.md](0001-memory-scopes.md) | Scoped memory with portable project identity | Accepted |
| [0002-session-event-log.md](0002-session-event-log.md) | SQLite index + JSONL session event log | Accepted |

## Adding an ADR

1. Copy the template below into `adr/NNNN-short-title.md` (next number).
2. Keep it short: Context → Decision → Consequences.
3. Update the index above.

### Template

```markdown
# NNNN. Short title

Status: proposed | accepted | superseded by NNNN

## Context

The problem and the constraints that matter.

## Decision

What we chose to do.

## Consequences

What becomes easier, what becomes harder, and what we accepted.
```
