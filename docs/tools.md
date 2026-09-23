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
| `web` | `search` via a replaceable provider; `fetch` as bounded HTTP(S) GET and cleaned Markdown/text | `Network` |
| `view_image` | Native image content from a workspace file, when the model supports vision | `FilesystemRead` |

The registry is assembled per composition root (`cli::tools`): the built-ins,
then discovered MCP proxies.

### Web

`web` accepts `operation: "search"` with `query` and optional `limit` (1–20),
or `operation: "fetch"` with an HTTP(S) `url`. Search returns
`title`/`url`/`snippet`/`source` records. The default search adapter uses Brave
Search and needs `BRAVE_SEARCH_API_KEY`; embedders can provide another
`SearchProvider`. Fetch sends only GET, follows up to five redirects, times out
after 15 seconds, rejects non-text content, limits the body to 2 MB, and
returns at most 100,000 characters of converted Markdown/text. It does not
execute browser JavaScript.

### LSP through MCP

AX does not include an LSP client or start language servers directly.
Configure a separate MCP server that exposes language-server operations; AX
discovers and calls its tools through the ordinary `mcp` gateway or discovered
MCP proxies. The available operations, input format, and edit behavior depend
on that server. See [mcp.md](mcp.md) for configuration and permission behavior.

### Images

`view_image` checks the canonical workspace path, file signature and a 10 MB
limit for PNG, JPEG, WebP and GIF. It returns an image content part, which the
OpenAI Responses provider sends as a native `input_image` in the tool result.
The kernel hides this tool when the active provider reports no vision support.
Image bytes are retained in raw session metadata; context budgeting uses an
approximate image token cost because API image tokenization depends on image
size and provider processing.

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
| Built-in tools | `crates/tool/src/shell.rs`, `filesystem.rs`, `patch.rs`, `search.rs`, `web.rs`, `view_image.rs` |
| `Capability`, `PermissionStore` | `crates/tool/src/permission.rs` |
| Telemetry | `crates/tool/src/telemetry.rs` |
| Registry assembly, approval wiring | `crates/cli/src/main.rs` |
| `/permissions`, `/tools` views | `crates/cli/src/tui/commands.rs`, `crates/cli/src/tui/commands/catalogs.rs` |
