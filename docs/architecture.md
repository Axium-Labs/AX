# AX Architecture

This document describes the overall system architecture of AX: the crate
layout, the agent loop, and the cold-start path. Topic-specific details live
in their own documents — see [docs/README.md](README.md) for the full index.

## Design boundaries

AX is an **agent runtime kernel**, not a chat personality layer. The kernel is
responsible for context, models, tools, events, permissions, and the execution
loop; skills and the caller decide task behavior. Core dependencies stay
one-directional and modules can be replaced independently.

```text
cli ───────────────┬──> runtime-core ──> model
                   │         └─────────> tool
                   ├──> skill
                   ├──> memory
                   └──> mcp ───────────> tool (proxy implementation)
```

## Crate responsibilities

| Crate | Responsibility |
|---|---|
| `model` | Provider-neutral `ModelProvider`, messages, tool calls, response types. One streaming entry point (`complete_stream`) with a non-streaming fallback. |
| `tool` | `Tool`, `ToolRegistry`, JSON Schema, `SafetyLevel`, plus built-ins: `shell`, `filesystem`, `patch`, `search`. Owns the `PermissionStore`. |
| `runtime-core` | The model → tool → model agent loop, `AgentEvent` stream, context selection, `ContextBudget`, compaction, and `AgentSupervisor` for bounded-concurrency tasks. |
| `mcp` | MCP client for stdio / Streamable HTTP / WebSocket, lazy connection, capability catalog, `McpToolProxy` and `McpGateway`. |
| `skill` | `skill.toml` metadata indexing and dependency-aware routing; `instructions.md` is loaded only when a route hits. |
| `memory` | SQLite session/message repository, effective-context snapshots, scoped facts, JSONL event streams, resume and compaction state. |
| `cli` | The only composition root: clap arguments, provider selection, lazy SQLite/Skill/MCP initialization, REPL, ratatui TUI, session commands, permission dialogs. |

### `model`

Defines the provider-neutral `ModelProvider` trait, message types, tool calls
and responses. `complete_stream` is the unified streaming entry point and
falls back to a non-streaming implementation by default. Existing providers:

- DeepSeek Chat Completions;
- OpenAI Responses API;
- Codex local-file auth + ChatGPT Codex Responses endpoint.

Providers also report their context window, which drives the kernel's
compression policy. Adding a local model means implementing the trait — no
agent-loop changes.

Credentials are persisted in `~/.ax/auth.json`: on Unix the file is written
with mode `0600`; on Windows there is no mode bit, so AX calls `icacls` to
remove inherited ACEs and grant the current user full control (best effort —
credential saving itself never depends on the tool succeeding).

### `tool`

Defines `Tool`, `ToolRegistry`, JSON Schema and `SafetyLevel`, and provides
`shell`, `filesystem`, structured `patch` (multi-hunk edits; any failing hunk
aborts the whole write) and `search` (line-scoped text search) to reduce shell
abuse.

Every tool declares its own permissions through `permission()` — a
`ToolPermission { capability, safety }` — and neither the kernel nor the UI
guesses permissions from tool-name strings (e.g. `mcp__` / `::`). The
`PermissionStore` (`tool::permission`) is the single permission source: the UI
and the runtime hold clones of the same `Arc<RwLock<..>>`, so a policy change
on either side takes effect immediately on the other. Session-scoped
temporary approvals (`allow_session`) are cleared automatically when a session
starts and cannot override an explicit Deny. The `telemetry` submodule
provides in-process, no-sensitive-data latency metrics (`Timer`/`snapshot`)
for the `/status` panel.

### `runtime-core`

Owns:

- The **agent loop** constrained by `ExecutionBudget` (max model steps, max
  tool calls, per-turn timeout, per-tool timeout — all configurable via CLI
  flags). When the budget or a timeout fires, hanging tool calls get a
  placeholder result so the message record stays well-formed.
- The **`AgentEvent` stream**: token deltas, tool state, compression events.
- **Context management**: `context::select_context` picks history for the
  model by token budget (not fixed message count) and never cuts through an
  unfinished tool-call round.
- **`ContextBudget`** (`runtime-core::budget`): one structure from which every
  context-space limit is derived — reserved output tokens, tool-schema
  estimate, skill reserve, memory reserve, session-summary share, recent-message
  budget and the compaction threshold. Modules no longer hard-code their own
  character counts or fixed fractions of the raw window. See
  [context.md](context.md).
- **Capacity-aware compaction**: compression only affects what is fed to the
  model; the latest cumulative summary is upserted per session and the
  watermark advances, while raw messages in the `messages` table are never
  deleted. See [memory.md](memory.md) and [storage.md](storage.md).
- **`AgentSupervisor`**, running tasks with independent contexts and bounded
  concurrency.

The core has no fixed system prompt. Only compaction uses a focused,
impersonal summarization instruction.

### `mcp`

Reads only server configuration metadata until a server's tools are actually
needed. Implements stdio, Streamable HTTP and WebSocket transports, with
`initialize` negotiation, modern stateless protocol, paginated `tools/list`,
`tools/call`, request timeouts and process cleanup.

`McpToolProxy` turns a remote schema into an ordinary `Tool`, so the agent
loop needs no MCP branch. Dynamic names are `mcp__server__tool` to avoid
cross-server collisions. `McpGateway` (a single `mcp` tool) additionally
exposes a lightweight capability catalog: even a never-connected server's
name, description and declared capabilities are visible to the model via
`action="catalog"`; only `list_tools`/`call` establish a real connection. See
[mcp.md](mcp.md).

### `skill`

On first use, indexes only the `skill.toml` metadata: name, description,
trigger keywords, required tools. Routing returns every candidate whose
required tools are available (`route_candidates`), not just the top-1 keyword
hit, and the caller decides; only after a hit and dependency check is the
corresponding `instructions.md` read. The directory-package format can
naturally be downloaded and installed by a future marketplace. See
[skills.md](skills.md).

### `memory`

SQLite stores raw session messages, effective-context snapshots and scoped
facts. Global facts live in the AX home; Project and Session facts live in the
selected project database. Project ownership is a UUID persisted in
`.ax/project.json`, independent of the absolute workspace path and
`--data-dir`.

Explicit `remember key=value` declarations default to Session scope. Natural
language intent is handled by the main model through a session-bound `memory`
tool; code validates scope ownership, quoted user provenance, credential
patterns, and optimistic-update preconditions. Retrieval uses lexical
relevance, update time, explicit Global inclusion settings and the shared
token budget.

Every complete conversation message is checkpointed before execution
advances. Resume queries pages after the snapshot watermark and marks
interrupted tool calls without replaying their side effects. Compression
snapshots stay session-local; raw messages are retained. See
[memory.md](memory.md) and [storage.md](storage.md).

### `cli`

The only composition root. Owns clap arguments, provider selection, lazy
SQLite/Skill/MCP initialization, the REPL, the ratatui TUI, session commands
and permission dialogs. No other crate depends on terminal UI.

`cli::providers` is the single definition of "which providers are
configured" (AX's own `auth.json`, conventional environment variables, and
explicit legacy Codex auth paths). Both CLI startup resolution
(`model_selection`) and the TUI's `/model` catalog refresh
(`tui::catalog_refresh`) delegate to it, instead of each maintaining a
drift-prone copy.

## Cold-start path

```text
parse args
   ↓
construct lightweight ReplState
   ↓
show CLI / recent session metadata
   ↓ first task or explicit command
open selected session / build provider / index skills / connect one MCP server
```

Guarantees:

1. The provider is built only on the first model call.
2. Skills are indexed (metadata only) only on first routing or `/skills`.
3. `instructions.md` is read only when a route hits.
4. MCP processes/connections are established only on `/mcp tools <server>`;
   the capability catalog (names/descriptions/declared capabilities) is
   visible to the model without connecting.
5. Memory restores only the current session's summary; historical messages
   are selected by the model's token budget (not a fixed count), and raw
   history stays complete in SQLite/JSONL.
6. SQLite migrations create only small base tables and necessary indexes.

## Agent loop

```text
user task
  → lazy Skill routing
  → retrieve memory and checkpoint user input
  → context pressure check / layered compression
  → model streaming request
  → final text ──────────────→ persist and finish
  → tool calls
      → permission check
      → execute built-in or MCP proxy
      → append tool result
      → checkpoint result
      → context pressure check / next model step
```

The loop is bounded by `ExecutionBudget` (defaults: 64 model steps, 128 tool
calls, 600s per-turn timeout, 120s per-tool timeout; overridable with
`--max-steps` / `--max-tool-calls` / `--turn-timeout-secs` /
`--tool-timeout-secs`). When the budget or a timeout fires, any hanging tool
call gets an interruption placeholder so the message record stays valid. All
user, assistant, tool, MCP and skill state messages are written to the current
session. Compression keeps skill system context and the full raw history and
only replaces older conversation/execution records fed to the model with a
summary. Every model step and tool call records latency metrics visible in
`/status`.

## Extension points

| To add… | You touch… |
|---|---|
| A new model | Implement `ModelProvider` |
| A new built-in tool | Implement `Tool` and register it |
| A new MCP transport | Implement the transport request boundary |
| A new skill | Add a `skill.toml` + `instructions.md` package |
| A new UI | Consume `AgentEvent` and call the kernel |
| New storage | Keep session/message repository semantics; don't leak the database into core |

Step-by-step guidance lives in [development.md](development.md).
