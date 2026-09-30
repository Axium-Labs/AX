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

### Tool use and result protocol

The CLI supplies a strategy without changing permissions or the dependency DAG.
Known independent search/read calls must be emitted in one response. Prefer
targeted exact search, then numbered `filesystem.read` ranges (`start_line`,
`end_line`). Failure recovery uses minimal diagnostics, a local repair and the
smallest relevant check before required full tests.

`patch` addresses original 1-based coordinates with `start_line`, `delete_count`,
`new_text` and optional `expected_lines`. All hunks validate before writing,
preserve CRLF, and report conflict locations with local context. Repeated text
is supported; unique `old_text` matching is no longer the edit API. Missing paths
return nearby candidates without a recursive search. Use `filesystem.write`
for new files.

Text results are stored as lossless `ToolResult` envelopes: `status`
(`success`/`error`), `summary`, `diagnostics`, `raw_output`, `truncated`. Model
projections derive from `ContextBudget`, retain key errors and remove redundant
compiler warnings. Raw checkpoints remain authoritative.
`tool_output(call_id, start_line, end_line)` reads retained raw ranges on demand,
including after restore. Typed DAG references still resolve the original value.

Nonzero shell exits fail. ACP live updates and replay retain failure status,
result envelopes, original call IDs and tool names. ACP `turn_changes` lists
changed files using current Git diff counts and is persisted through the existing
history; it never stages/reverts files and is excluded from model context. Counts
may include pre-existing edits to the same changed file. `AX_EVENT_LOG` optionally
records runtime JSONL for measurement without normal startup work. The workspace
`benchmark/README.md` documents the fixed local benchmark and limitations.

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

Queries are executed concurrently against one `SearchProvider`. The default
total target is `limit` distinct valid URLs; optional `target_results` sets a
different target (up to `limit * query_count`). As soon as the target is met,
remaining futures are dropped. Results are merged in original query order,
and `cancelled_queries` identifies unfinished queries; cancellation is not a
failure. A short result is allowed when every query completes before reaching
the target. A partial failure never discards successful requests: the response
reports `succeeded`, `failed` and `errors`, and the call fails only when every
query or URL fails.

URL retrieval uses at most three in-flight requests per call. Optional
`target_pages` returns as soon as that many pages succeed, dropping the rest
and reporting `cancelled_urls`. Its default is all explicitly requested URLs;
request a smaller target when only a subset is needed. Completed pages and
their character budgets remain in original URL order. Dropping a request
cancels local reads and retries; it cannot undo work already received by a
remote server.

Search results from completed queries are merged, canonicalized (fragment and known
tracking parameters dropped, trailing slash trimmed) and deduplicated by URL.
The first occurrence keeps its position in query order, so the highest-ranked
copy of a URL is the one returned, and each record carries the
`matched_queries` that surfaced it. Records are
`title`/`url`/`snippet`/`source`/`matched_queries`. Without any API key, the
default adapter uses DuckDuckGo HTML search. Configuring `BRAVE_SEARCH_API_KEY`
selects Brave as the preferred provider; failures or empty Brave responses
fall back to DuckDuckGo. Embedders can still supply another `SearchProvider`,
or override built-in endpoints with `SearchConfig`. HTML results are parsed
with the HTML DOM parser, tracking redirects are unwrapped, and advertisements
and non-HTTP(S) links are excluded. Challenges are reported as errors rather
than fabricated results. Search never fetches destination page bodies.

The shared HTTP client retains a connection pool, HTTP Keep-Alive, 30-second
TCP keepalive, and a bounded 60-second DNS cache using the system resolver.
Concurrent lookups for the same name are coalesced. The default per-attempt
connect/read/total timeouts are 2/3/8 seconds. An injected client retains its
own transport settings. Only timeout, 429 and 5xx responses are retried, with
one retry after 100 ms; 401/403/404 are never retried. A retry failure is returned
with its complete error chain. Proxy settings and TLS verification still use
reqwest defaults.

Fetch sends only GET, follows up to five redirects,
rejects non-text content, limits each body to 2 MB, and removes comments and
`script`/`style`/`nav`/`footer`/`aside` noise before converting HTML to
Markdown. Every page returns `url`, `title` (when the page has one),
`content_type`, `content` and `truncated`; a page returns at most 20,000
characters and one call at most 60,000 characters, always reported through
`truncated: true` instead of silently cutting content. It does not execute
browser JavaScript.

Fetch errors include `url`, `error` (the complete joined error chain), `kind`,
`reason` (a short display label), and `source_chain` (outer error through root
cause). `kind` is one of `dns`, `connect`, `timeout`, `tls`, `proxy`, `redirect`,
`http_status`, or `unknown`; HTTP status errors also include `status_code`.
reqwest's typed status, redirect, timeout and connect flags take precedence;
DNS, TLS and proxy errors are recognized from source messages, with `unknown`
as the fallback. URLs are not used for classification. An all-failed fetch
remains a tool error and carries the same structured records in its diagnostic
payload. Partial failures keep their records in the successful result's
`errors` array.

The TUI shows a short row such as `web.fetch · failed (connect timeout)`.
Ctrl+O expands/collapses the latest fetch diagnostic, showing the full URL
and every underlying error. The latest diagnostic remains available after its
original row enters terminal scrollback. ACP completion updates expose these
records in `rawOutput.errors`.

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

Network timings add `web.http.dns`, `web.http.connect`, `web.http.ttfb`, and
`web.http.total`, plus DNS cache hit, retry, provider fallback and cancellation
counters. DNS is actual resolver duration; connect is the measured connector
service duration (including DNS, proxy and TLS when required); TTFB measures
response-header arrival because reqwest does not expose the first socket byte.
Total covers all attempts, backoff and body reads. These phase measurements
overlap and must not be added together. Reused connections and literal IPs may
perform no DNS lookup or connection establishment. No URL, query or credential
is stored in telemetry. See [web-latency.md](web-latency.md) for the benchmark.

## Concurrency

The Runtime builds a dependency DAG for every model tool-call round and runs
ready independent calls with bounded concurrency (default four, configurable
through `AgentKernel::with_tool_concurrency`, capped at 64). It uses in-task
futures rather than spawning a task per model call. Tool permission decisions
remain unchanged; interactive approvals are serialized while approved work
runs concurrently.

`Tool::resources(input)` declares read/write effects. Filesystem read/list,
search and image reads share path leases; filesystem writes and patches acquire
exclusive path leases. Canonical paths, Windows case normalization and directory
overlap prevent aliases and parent/child accesses from bypassing conflicts.
Memory uses its actual SQLite database path, so writes to that database are
serialized across scopes and kernels; reads can overlap. MCP catalog reads can
overlap; list-tools modifies the relevant manager resource. Shell scripts,
including Git commands, and tools without explicit effect declarations hold a
global exclusive lease because their affected resources cannot be safely inferred
from command text, names or permission categories. A read-only tool declares its
effects explicitly to opt into concurrency. Leases are process-wide, including
forked kernels, and are released on success, error, timeout or cancellation.
They coordinate AX tool invocations, not external processes editing those paths.

Arguments may contain typed references such as
`{"$tool_result":"producer-id","pointer":"/url"}`. The runtime detects the
dependency, waits for the producer, and substitutes the referenced JSON value
(or the whole text/JSON result when no pointer is provided). The optional
reserved `_ax_depends_on` array adds explicit success dependencies and is stripped
before permission checks and execution. These forms are exposed through runtime
tool schemas; no scheduling decision relies on a prompt. Unknown IDs, duplicate
IDs and cycles are rejected before execution. Failed input dependencies suppress
dependent operations; a failed earlier resource write does not suppress a later
independent call that merely shares the resource.

Tool lifecycle events carry the original `tool_call_id`, including same-name
calls. Completed results are checkpointed as they finish, preserving raw history,
then returned together to the next model request in original call order. Tool
budgets are checked before starting a round, and interrupted calls retain the
existing recovery placeholders.

## Reference

| Concern | Code |
|---|---|
| `Tool`, `ToolRegistry`, `SafetyLevel` | `crates/tool/src/lib.rs` |
| Built-in tools | `crates/tool/src/shell.rs`, `filesystem.rs`, `patch.rs`, `search.rs`, `web.rs`, `view_image.rs` |
| `Capability`, `PermissionStore` | `crates/tool/src/permission.rs` |
| Telemetry | `crates/tool/src/telemetry.rs` |
| Runtime scheduling and resource locks | `crates/core/src/scheduler.rs`, `crates/tool/src/resources.rs` |
| Registry assembly, approval wiring | `crates/cli/src/main.rs` |
| `/permissions`, `/tools` views | `crates/cli/src/tui/commands.rs`, `crates/cli/src/tui/commands/catalogs.rs` |
