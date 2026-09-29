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
| `web` | `search` up to 4 concurrent queries via a replaceable provider; `fetch` up to 6 concurrent HTTP(S) GETs as cleaned Markdown/text; merged, deduplicated, partial failures tolerated | `Network` |
| `view_image` | Native image content from a workspace file, when the model supports vision | `FilesystemRead` |

The registry is assembled per composition root (`cli::tools`): the built-ins,
then discovered MCP proxies.

The TUI tool timeline shows a short description while each call runs (for
example the search query and path, file operation and path, or shell command).
After completion it keeps the detail visible briefly, then folds it into a
single success or failure row.
CLI `run` prints the same activity description to stderr.

### Web

`web` accepts `operation: "search"` with `queries` (1–4 strings) and optional
`limit` (1–20 per query), or `operation: "fetch"` with `urls` (1–6 HTTP(S)
URLs). The legacy singular `query` and `url` keys remain as aliases and are
normalized into the same lists, so a single-request call keeps working.

Queries are executed concurrently against one `SearchProvider`, and URLs are
fetched concurrently; a call takes about as long as its slowest request rather
than the sum of all of them. A partial failure never cancels the successful
requests: the response reports `succeeded`, `failed` and an `errors` array, and
the call fails only when every query or URL fails.

Search results from all queries are merged, canonicalized (fragment and known
tracking parameters dropped, trailing slash trimmed) and deduplicated by URL.
The first occurrence keeps its position in query order, so the highest-ranked
copy of a URL is the one returned, and each record carries the
`matched_queries` that surfaced it. Records are
`title`/`url`/`snippet`/`source`/`matched_queries`; the default adapter uses
Brave Search and needs `BRAVE_SEARCH_API_KEY`, and embedders can provide
another `SearchProvider`.

Fetch sends only GET, follows up to five redirects, times out after 15 seconds,
rejects non-text content, limits each body to 2 MB, and removes comments and
`script`/`style`/`nav`/`footer`/`aside` noise before converting HTML to
Markdown. Every page returns `url`, `title` (when the page has one),
`content_type`, `content` and `truncated`; a page returns at most 20,000
characters and one call at most 60,000 characters, always reported through
`truncated: true` instead of silently cutting content. It does not execute
browser JavaScript.

```json
{
  "operation": "search",
  "queries": ["Rust 1.94 release", "Rust 1.94 changes"],
  "limit": 5
}
```

```json
{
  "operation": "fetch",
  "urls": ["https://example.com/a", "https://example.com/b"]
}
```

`queries` are executed concurrently, URLs are fetched concurrently, results are
deduplicated by URL, and partial failures do not cancel successful results.

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
(`Timer` / `snapshot` / `increment`) for the `/status` panel. Every model step
and tool call records a timer. Batched `web` calls add `web.search` and
`web.fetch` totals, per-item `web.search.query.N` and `web.fetch.url.N`
latencies, and `web.search.ok` / `web.search.failed` / `web.fetch.ok` /
`web.fetch.failed` counters.

## Concurrency

Parallel tool execution lives inside each tool, never in the agent loop. Tools
that may depend on each other (`shell`, `filesystem`, `patch`, `view_image`)
keep running one call at a time; the read-only network tool batches its own
requests. `web` is the only built-in that runs independent requests
concurrently.

## Reference

| Concern | Code |
|---|---|
| `Tool`, `ToolRegistry`, `SafetyLevel` | `crates/tool/src/lib.rs` |
| Built-in tools | `crates/tool/src/shell.rs`, `filesystem.rs`, `patch.rs`, `search.rs`, `web.rs`, `view_image.rs` |
| `Capability`, `PermissionStore` | `crates/tool/src/permission.rs` |
| Telemetry | `crates/tool/src/telemetry.rs` |
| Registry assembly, approval wiring | `crates/cli/src/main.rs` |
| `/permissions`, `/tools` views | `crates/cli/src/tui/commands.rs`, `crates/cli/src/tui/commands/catalogs.rs` |
