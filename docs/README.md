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
| [backup.md](backup.md) | Versioned axpack export/import, data boundaries and project remapping |
| [tools.md](tools.md) | Tools, permissions, safety |
| [mcp.md](mcp.md) | MCP integration: configuration, transports, lazy connection |
| [skills.md](skills.md) | Skills: package format, routing, enable/disable |
| [providers.md](providers.md) | Models, providers, authentication |
| [development.md](development.md) | Building, testing, extending and releasing AX |
| [adr/README.md](adr/README.md) | Architecture Decision Records (index) |

## TUI features

- **Project file references** — Type `@README` to fuzzy-search project files.
  Select one with Enter to include its text in the model context when sending
  the message. See [context.md](context.md).
- **Tool execution timeline** — Shows what a running tool is searching,
  reading, editing, or executing, then collapses to a result row. See
  [tools.md](tools.md).
- **Session picker** — `/resume` searches sessions across registered projects.
  Type to filter titles; use Ctrl+P to switch projects, Ctrl+R to rename,
  Ctrl+D to delete, and Ctrl+N to start a new session. See
  [context.md](context.md).
- **Automatic session titles** — New sessions get a short title from the first
  task. Titles can be changed in `/resume`. See [context.md](context.md).

## TUI interaction feedback

Streaming replies show new text promptly while combining dense updates into
short redraw frames. Completed Markdown blocks and messages are cached at the
current terminal width; only the active tail is re-rendered. The active reply
has a cursor until it ends. When reading earlier transcript lines, the footer
reports new output and End returns to the latest text. Tool details remain
visible briefly after completion before they
collapse. Pickers reveal their contents quickly, file and slash suggestions
highlight keyboard movement, and session restoration displays a loading state
while it reads history. Slash command hints, descriptions, and selected text use
fixed high-contrast colors so they remain legible across terminal themes; the
slash popup keeps its navigation hint visible while scrolling.

The terminal palette uses a black canvas with quiet slate surfaces, teal for
focus, and separate green, amber, and coral status colors. Text colors remain
explicit so terminal theme defaults cannot make command hints or tool output
disappear.

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
