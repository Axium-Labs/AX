# Tools, Permissions & Safety

This document covers AX's built-in tools, how tools declare their permission
requirements, and how the runtime and UI enforce them.

## Tool model

A **`Tool`** (`crates/tool`) is a named callable with a JSON input schema. The
`ToolRegistry` collects the tools available to one agent. The agent loop is
tool-agnostic: MCP remote tools are proxied into ordinary `Tool`s, so the loop
never branches on where a tool came from.

Each tool declares its own permissions through `permission()`, returning a
`ToolPermission { capability, safety }`. Neither the kernel nor the UI guesses
permissions from tool-name strings (e.g. `mcp__`/`::`).

| Field | Meaning |
|---|---|
| `capability` | The capability class the tool exercises |
| `safety` | `SafetyLevel::Safe` or `SafetyLevel::RequiresApproval` |

## Built-in tools

| Tool | Purpose | Capability |
|---|---|---|
| `shell` | Run a shell command | `Shell` / `Process` |
| `filesystem` | Read/write files | `FilesystemRead` / `FilesystemWrite` |
| `patch` | Structured multi-hunk edits; any failing hunk aborts the whole write | `FilesystemWrite` |
| `search` | Line-scoped text search | `FilesystemRead` |

The registry is assembled per composition root (`cli::tools`): the built-ins,
then discovered MCP proxies.

## Capabilities

The shared vocabulary of permission checks:

| Capability | Key |
|---|---|
| Shell | `shell` |
| Filesystem read | `filesystem-read` |
| Filesystem write | `filesystem-write` |
| Network | `network` |
| MCP | `mcp` |
| Process launch | `process` |

## PermissionStore

`PermissionStore` (`tool::permission`) is the **single** permission source.
The UI and the runtime hold clones of the same `Arc<RwLock<..>>`, so a policy
change on either side takes effect immediately on the other.

- **Default policy**: `filesystem-read` and `network` are `Allow`; everything
  else defaults to `Ask`.
- **Decisions**: `Allow` / `Ask` / `Deny` per capability.
- **Session grants** (`allow_session`): temporarily upgrade an `Ask` to
  `Allow` for the rest of the session. They are cleared automatically when a
  session starts or resumes and can never override an explicit `Deny`.
- **Approval policy**: `ApprovalPolicy` implementations (`AllowAll`,
  `DenyDangerous`) read the same store; `--allow-dangerous` selects
  `AllowAll`. `DenyDangerous` rejects `RequiresApproval` tools.

## User controls

- **`/permissions`** — view and change the runtime's tool capability policies.
  Selecting a capability opens `Allow` / `Ask` / `Deny`; choosing updates the
  shared `PermissionStore`, and the runtime reads the same store, so updates
  affect subsequent approval checks.
- **`/tools`** — inspect built-in tools, discovered MCP proxies and the lazy
  `mcp` gateway. Details show the real description and JSON input schema, plus
  server and remote name for MCP proxies. Browsing never connects servers and
  never claims a static permission decision for every possible call.

## Telemetry

`tool::telemetry` provides in-process, no-sensitive-data latency metrics
(`Timer` / `snapshot`) for the `/status` panel. Every model step and tool call
records a timer.

## Reference

| Concern | Code |
|---|---|
| `Tool`, `ToolRegistry`, `SafetyLevel` | `crates/tool/src/lib.rs` |
| Built-in tools | `crates/tool/src/shell.rs`, `filesystem.rs`, `patch.rs`, `search.rs` |
| `Capability`, `PermissionStore` | `crates/tool/src/permission.rs` |
| Telemetry | `crates/tool/src/telemetry.rs` |
| Registry assembly, approval wiring | `crates/cli/src/main.rs` |
| `/permissions`, `/tools` views | `crates/cli/src/tui/commands.rs`, `crates/cli/src/tui/commands/catalogs.rs` |
