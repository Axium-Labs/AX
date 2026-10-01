# Architecture Decision Records

ADRs record design decisions with lasting consequences: the context, the
decision, and the trade-offs accepted. They make the *why* behind the code
discoverable.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001-memory-scopes.md](0001-memory-scopes.md) | Scoped memory with portable project identity | Accepted |
| [0002-session-event-log.md](0002-session-event-log.md) | SQLite index + JSONL session event log | Accepted |
| [0003-portable-axpack.md](0003-portable-axpack.md) | Versioned portable AX data packages | Accepted |
| [0004-acp-control-plane.md](0004-acp-control-plane.md) | ACP adapter at the AX CLI boundary | Accepted |
| [0005-tool-round-scheduler.md](0005-tool-round-scheduler.md) | Dependency-aware tool-round scheduling | Accepted |
| [0006-skill-evolution.md](0006-skill-evolution.md) | Skill evolution outside the runtime kernel | Accepted |
| [0007-installation-storage.md](0007-installation-storage.md) | Installation-owned persistent state | Accepted |

- [0008 — Durable lightweight Task Queue](0008-durable-task-queue.md)
- [0009 — Goal-bound execution invariants](0009-execution-invariants.md)
- [0010 — Optional model-selected subagents](0010-optional-subagents.md)

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

- [0011: Scoped capabilities](0011-scoped-capabilities.md) — one shared Global/Project registry for Skill, MCP and Agent management.
