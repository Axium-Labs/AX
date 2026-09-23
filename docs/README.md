# AX Documentation

Documentation for the AX terminal agent. Each topic has one focused document;
design decisions that shaped the code are recorded as ADRs in
[adr/](adr/README.md).

## Documentation map

| Document | Topic |
|---|---|
| [architecture.md](architecture.md) | System architecture: crate responsibilities, agent loop, cold-start path |
| [memory.md](memory.md) | Memory: scopes, project identity, retrieval, persistence |
| [context.md](context.md) | Context budgeting, compression, session resume |
| [storage.md](storage.md) | Storage: SQLite schema, JSONL event streams, data directories, migration |
| [tools.md](tools.md) | Tools, permissions, safety |
| [mcp.md](mcp.md) | MCP integration: configuration, transports, lazy connection |
| [skills.md](skills.md) | Skills: package format, routing, enable/disable |
| [providers.md](providers.md) | Models, providers, authentication |
| [development.md](development.md) | Building, testing, extending and releasing AX |
| [adr/README.md](adr/README.md) | Architecture Decision Records (index) |

## How to read

- **New users** — start with the repository [README](../README.md):
  install, first session, providers.
- **Codex / AI agents** — start at [AGENTS.md](../AGENTS.md); it routes to
  the right documents for a task.
- **Contributors** — read [architecture.md](architecture.md) first, then the
  topic document for the area you touch, then [development.md](development.md)
  for build/test conventions.
- **Deep dives** — [storage.md](storage.md) and [adr/](adr/README.md) contain
  the storage and memory design rationale.

## Writing docs

- One topic per file; cross-reference instead of duplicating.
- Keep language consistent with the repository (English).
- Record decisions with lasting consequences as ADRs — see
  [adr/README.md](adr/README.md).
- Update this map when adding or renaming a document.
