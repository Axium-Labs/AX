# AX documentation

This directory documents current AX behavior, architecture and development.
Version-specific changes are recorded under [release notes](release-notes/).
Long-lived design decisions are recorded as [ADRs](adr/README.md).

## Documentation map

See [platforms.md](platforms.md) for cross-platform builds, shell settings and
capability limits.

Native desktop automation and its AXCrew settings contract are documented
in [computer-use.md](computer-use.md).
Independent application/site access, browser sessions and sandbox/approval boundaries
are documented in [host-permissions.md](host-permissions.md).

| Document | Topic |
|---|---|
| [agent-runtime.md](agent-runtime.md) | The runtime contract: request/context/capability boundary, turn flow, trigger conditions |
| [coding-harness.md](coding-harness.md) | Execution policy, bounded error-driven handoff review, concrete work, workspace specs and durable artifacts |
| [long-project.md](long-project.md) | Optional durable working context, scoped templates, safe Session handoff and recovery |
| [goal.md](goal.md) | Optional target execution, validation, lifecycle, budgets and recovery |
| [agent-loop.md](agent-loop.md) | Continuation state, streaming completion, error-driven handoff feedback and optional Goal |
| [architecture.md](architecture.md) | System architecture: crate responsibilities, agent loop, cold-start path |
| [distributed-collaboration.md](distributed-collaboration.md) | Optional durable cross-Host Tasks, Events, Artifacts, Workflow State and AX worker |
| [acp-crew.md](acp-crew.md) | ACP adapter and AX Crew device bridge |
| [memory.md](memory.md) | Memory: scopes, project identity, retrieval, persistence |
| [context.md](context.md) | Context budgeting, compression, session resume |
| [storage.md](storage.md) | Storage: SQLite schema, JSONL event streams, data directories, migration |
| [backup.md](backup.md) | Versioned axpack export/import, data boundaries and project remapping |
| [tools.md](tools.md) | Tools, permissions, safety |
| [security.md](security.md) | Runtime Workspace Sandbox, OS enforcement and escape tests |
| [web-search-architecture.md](web-search-architecture.md) | Web search: two-tier native-first routing, provider capabilities, configuration |
| [mcp.md](mcp.md) | MCP integration: configuration, transports, lazy connection |
| [skills.md](skills.md) | Skills: package format, routing, enable/disable |
| [capabilities.md](capabilities.md) | Shared Global/Project registry and Skill/MCP/Agent/Mod management |
| [mods.md](mods.md) | Executable JavaScript Mods, loading, hooks, commands, tools and compatibility |
| [evolution.md](evolution.md) | Low-frequency Experience learning and evolved Skill lifecycle |
| [providers.md](providers.md) | Models, providers, authentication |
| [development.md](development.md) | Building, testing, extending and releasing AX |
| [release-notes/0.3.15.md](release-notes/0.3.15.md) | Current release |
| [adr/README.md](adr/README.md) | Architecture Decision Records (index) |

Older AX release notes are kept by version in the same `release-notes/`
directory. The local [Web Search scheduling benchmark](web-latency.md) is a
maintainer reference, not part of the product feature guide.

## How to read

- **New users** — start with the repository [README](../README.md):
  install, first session, providers.
- **Codex / AI agents** — start at [AGENTS.md](../AGENTS.md); it routes to
  the right documents for a task.
- **Contributors** — read [architecture.md](architecture.md) first, then the
  topic document for the area you touch, then [development.md](development.md)
  for build/test conventions.
- **Deep dives** — [storage.md](storage.md), the Web Search audit and
  [adr/](adr/README.md) contain implementation and design references.

## Documentation conventions

- One topic per file; cross-reference instead of duplicating.
- Keep language consistent with the repository (English).
- Record decisions with lasting consequences as ADRs — see
  [adr/README.md](adr/README.md).
- Update this map when adding or renaming a document.
- Keep test results, machine-specific paths and task progress in `logs/`, not in
  current feature documentation. Record only reproducible, user-relevant limits
  in feature documents.
- Web Search implementation details and validation limits are in
  [web-search-architecture.md](web-search-architecture.md) and
  [web-search-audit.md](web-search-audit.md).
