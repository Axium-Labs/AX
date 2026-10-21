# Tools, Permissions & Safety

This document covers AX's built-in tools, how tools declare their permission
requirements, and how the runtime and UI enforce them.

## Computer Use

The native `desktop` tool provides real accessibility reads and control
actions. `ax computer-use` owns its global configuration; AXCrew edits the same
state. It defaults on, preserves explicit disable settings, enforces node/image
limits, redacts passwords and skips password-bearing window screenshots.
See [computer-use.md](computer-use.md) for
the exact actions, permissions, platform boundaries and screenshot limitations.

## Agent environment and Crew terminal shell

The `desktop` and `browser` tools use independent `ComputerUse` and `BrowserUse`
capabilities and an `AuthorizedHost` execution boundary. File/terminal sandboxing,
target app/site grants and action approval are enforced separately; unknown host
access is not covered by `--allow-dangerous`. See [host-permissions.md](host-permissions.md)
for the CLI, exact target identities, browser runtime and current compatibility limits.

`ax environment` shows execution settings. On Windows, `ax environment native`
runs AX directly; `ax environment wsl` runs Linux AX in the default WSL
distribution. WSL selection first checks `~/.local/bin/ax --version`; install
the Linux AX binary there before selecting WSL. `/environment` opens the same
settings in the TUI. Changes take effect on the next AX launch.

On Linux/macOS, integrated terminal choices are `default`, `sh`, `bash`, `zsh`
and `fish`; Windows retains its existing shell choices. These settings affect
new Crew terminals, not the agent shell tool's POSIX contract. See
[platforms.md](platforms.md) for shell resolution, installation and limits.

The Windows launcher forwards stdio, exit status, the workspace and path flags
to Linux AX. It maps the shared `AX_HOME` and ACP session `cwd` through
`wslpath`. Agent shell commands use the Linux runtime's shell in WSL.

`ax environment --terminal-shell powershell|cmd|git-bash|wsl` independently
selects the shell for new Crew integrated terminals. Crew exposes both settings
under Settings > AX and validates the chosen terminal program before saving.
Existing processes and terminals keep their current environment.

## Tool model

Each `shell` call starts a fresh process in the session workspace, or in its bound
child workspace. Changing location in one call does not change subsequent calls.
`_ax_execution.scope` declares the step's resource scope; it does not select the
shell working directory. Commands targeting a nested project must explicitly
change location within that command or use a program's directory option, such as
`git -C ax status`. Shell results report their starting `working_directory` along
with exit status, stdout and stderr, so errors can be attributed to the directory
that was actually checked. A root-directory Git error does not establish whether
its subdirectories are repositories.

Shell completion follows the actual shell process exit, independently of pipe
EOF. Stdin feeding and stdout/stderr collection run concurrently. Once the shell
exits, AX allows one second for pending I/O to drain; then it closes retained
handles and returns captured bytes with the shell's real exit status. If a
background descendant keeps a pipe open, the result includes
`output_capture_incomplete: true` and an explanation. Captured text can be partial.
The drain deadline does not limit foreground command runtime or stop background
processes. Normal EOF has no extra delay. Cancellation drops the owned pipe
futures; it leaves no detached output-reader tasks behind. The existing command
timeout/permission policy and nonzero-exit error handling still apply.

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

Tool behaviour guidance is capability-scoped and lives with the capability. Each
tool may declare `guidance()` (`crates/tool/src/lib.rs`), a short statement of
how to use it correctly *once the model has decided to use it*; the kernel
assembles these into `[ax-capability-guidance]`. Guidance never implies the
model should choose a tool. There is no global "tool use strategy" prompt.

Alongside it, the runtime prompt states that the current user request
defines the work and that capabilities are options, not obligations. Queue and
delegation discipline live in the `[ax-coding-harness]` policy. See
[agent-runtime.md](agent-runtime.md).
Communication guidance still applies: report related tool calls as one work
phase, avoid narrating each routine result, and finish with one coherent answer.
This guidance applies to CLI/TUI and ACP sessions used by AXCrew; it does not
discard model prose or tool history. AXCrew groups and folds that history in its
own presentation layer.

| Tool | Purpose | Capability |
|---|---|---|
| `shell` | Run builds, tests, Git operations and command-line workflows | `Shell` / `Process` |
| `filesystem` | Read/list/write a path you already know | `FilesystemRead` / `FilesystemWrite` |
| `find_files` / `glob` | Discover files and directories by name, path or extension; never reads content | `FilesystemRead` |
| `patch` | Structured multi-hunk edits; any failing hunk aborts the whole write | `FilesystemWrite` |
| `search` | Line-scoped content, symbol and regex search (grep) | `FilesystemRead` |
| `web` | `search` up to 4 concurrent queries via a replaceable provider; `fetch` up to 6 concurrent HTTP(S) GETs as cleaned Markdown/text; merged, deduplicated, partial failures tolerated | `Network` |
| `view_image` | Native image content from a workspace file, when the model supports vision | `FilesystemRead` |

The registry is assembled per composition root (`runtime::tools` in
`crates/cli/src/runtime/builder.rs`): the built-ins,
then discovered MCP proxies.

### Local discovery and search

`find_files` (also registered as `glob`) and `search` are one shared
implementation, not two agents' worth of logic:

- `find_files`/`glob` only answers "which paths exist": `pattern` is a glob
  (`**/*.jsonl`, `src/**/*.ts`, `*.toml`; a bare name pattern matches at any
  depth), plus `include`/`exclude`, `type` (`file`/`dir`/`any`), `max_depth` and
  `max_results`. It never opens a file.
- `search` only answers "where does this text/symbol/regex appear": `mode`
  (`literal`/`regex`), `case_sensitive` (default true), `include`/`exclude`,
  `context_lines`, `max_results` and `output_mode` (`content` or
  `files_with_matches`). Matching is per line.
- Both accept `path` (canonical) or `root` (alias); omitting it means the bound
  workspace root. Results are workspace-relative, `/`-separated paths, so a
  returned path can be passed straight to `filesystem.read`.
- Both skip `.git`, `target`, `node_modules`, `.venv`/`venv`, `dist`, `build`,
  `__pycache__`, caches, `coverage`, `vendor`, `.ax` and `child-runs` by default,
  honour `.gitignore` rules plus `.axignore` even outside a git checkout, never
  follow symlinks and never read files above 4 MB.
- An existing scope with no match returns `success` with an empty `matches`
  array (`search`) or `paths` array (`find_files`/`glob`). A missing `path`/`root`
  returns the same `path_error` execution diagnostic as `filesystem`, including
  nearby candidate paths.
- Traversal is provided by the `ignore` crate — the walker ripgrep uses — and
  matching by `regex`, so AX carries its own implementation and never shells out
  to a user-installed `rg`.

Both tools declare their resolved scope as a **read-only** resource, so
independent discovery calls in one round run concurrently instead of queueing
behind the global write lock. One round also never walks the same scope twice:
an identical read-only discovery call is ordered after the first and reuses its
result (the duplicate still receives its own tool response). Its short
`reused_from` receipt says it is identical to the first call, retains `status`
and `truncated`, and omits the duplicate body/diagnostics. Same-round typed result
references follow the original receipt ID. This is state
reuse only — the runtime never chooses which tool to call, and never hardcodes a
search order.

Child agents get the identical registry. `SandboxedTool::fork_for_run` delegates
to the wrapped tool's `fork_for_run`, and the discovery tools carry the
workspace they are bound to, so a child is the same struct with `cwd` rebound to
`child.cwd`; `filesystem` read/list/write are rebound through `WorkspaceTool`
the same way. Path binding plus the runtime's declared-scope check plus the
sandbox keep a child inside its own workspace — no second search implementation
is involved.

### Tool use and result protocol

Tool descriptions supply default preferences without changing permissions or the
dependency DAG. Prefer direct reads/lists for known paths, `find_files`/`glob` for
unknown locations, and `search` for text, symbols or regexes. A known file may
still need a targeted regex/symbol/usage search. An empty result from an existing
scope is a successful observation; avoid identical searches without new evidence,
but changed files, revised patterns/scopes or explicit user verification can justify
another search.
Identical discovery calls within one round retain the existing result reuse.

Use shell for builds, tests, Git operations and command-line workflows. Prefer
dedicated tools for routine discovery; shell scans are appropriate when explicitly
requested, dedicated tools are unavailable, or native filters/pipelines are needed.
These preferences do not add tool-selection gates. Known independent reads/searches
can be batched when useful. Failure recovery uses relevant diagnostics, a local
repair and appropriate checks. Platform syntax checks and tool permissions apply.

`patch` addresses original 1-based coordinates with `start_line`, `delete_count`,
`new_text` and `expected_lines` (required when `delete_count > 0`). Read enough relevant context before
editing, including surrounding functions and callers when needed. All hunks
validate before writing and report conflict locations with local
context. Each replacement follows the last replaced line's own LF/CRLF ending;
an insertion follows its preceding line (the first line when inserting at the
start, LF for an empty file). A nonempty replacement before surviving lines gains
a terminator when missing. An insertion after an unterminated final line first
terminates that line, using the preceding LF/CRLF ending (LF if none exists).
Unedited bytes otherwise keep their original endings, including
in mixed-ending files. Input and final file sizes use `MAX_FILE_BYTES`. Successful
receipts include `regions`: verbatim numbered text for each changed region plus
up to two lines on each side, using the final file's coordinates. Pure deletions
show the two lines on either side of their new boundary. Read these regions before
further edits; success does not establish that the file parses or builds. Repeated text is supported; unique `old_text` matching is no longer the
edit API. Missing paths return nearby candidates without a recursive search.
Prefer `patch` for localized edits. `filesystem.write` creates or replaces the
entire file, which suits requested full-file rewrites and generated artifacts;
inspect existing content before replacing it. It does not append and still uses
the existing write permission path. `filesystem.read` supports line ranges when
sufficient and full-file reads when context requires them. Both forms return
`N: line` with original 1-based file coordinates. The `N: ` prefix is display
metadata, not file content: omit it from patch text and `expected_lines`. Read
text retains the file's line endings (including CRLF), indentation, compact JSON
formatting and final newline.

Text results are stored as lossless `ToolResult` envelopes: `status`
(`success`/`error`), `summary`, `diagnostics`, `raw_output`, `truncated`. Model
projections derive from `ContextBudget`. Tools declare source text through
`ToolOutput::Verbatim { text, total_lines, start_line, truncated, line_truncated }` returned by
`Tool::execute_output`; its default implementation keeps existing text behavior.
The scheduler preserves this declaration in the envelope's additive `verbatim`,
`total_lines` and `start_line` fields. Missing fields in old snapshots default to
ordinary text behavior; the runtime never guesses source semantics from a tool
name or the content itself.

Successful verbatim results have empty `diagnostics`: words such as `error`,
`failed` or `conflict` in a file are source content. Their model projection does
not parse/pretty-print JSON or filter `warning:` lines. When the
`ContextBudget::tool_result_chars()` allowance is exceeded, the projection keeps
only complete leading lines and reports `total_lines`, `shown_through_line`,
`next_start_line` and `truncated: true`. Continue with `filesystem.read` using
that `start_line`. A single line larger than the allowance keeps only its
beginning, truncated on a UTF-8 character boundary: the view adds
`line_truncated: true` plus a short note, and `next_start_line` is the following
line when one exists; at EOF it is null. A complete requested range also
returns null even if later file lines exist: continuation is supplied only for
a budget/byte-truncated receipt. Use `search` to locate content further inside
such a line. The verbatim view carries only source fields — `verbatim`, `output`,
`total_lines`, `start_line`, `shown_through_line`, `next_start_line`,
`truncated`, `line_truncated`, `raw_output_available` — because `status`,
`summary` and `diagnostics` carry no information for declared source text.
Ordinary text results extract inferred diagnostics only on failure; explicit
JSON `diagnostics` arrays are preserved on success too. JSON formatting and
compiler-warning cleanup remain unchanged. Raw checkpoints remain authoritative.
See [ADR 0031](adr/0031-tool-declared-verbatim-output.md).

`filesystem.read` checks the file size before loading it and reuses the 4 MiB
`MAX_FILE_BYTES` limit that `search` already applies. Above it, a whole-file read
returns `invalid tool input` with the actual size, the limit and a hint to use a
line range or `search`. A ranged read is still allowed: it streams the file, keeps
at most `MAX_FILE_BYTES` of numbered requested text, including prefixes. If the
limit cuts a source line, the tail is shortened at a UTF-8 character boundary and
marked `truncated` and `line_truncated`; no marker text is inserted into source.
The first read counts the rest without retaining it. Exact counts are cached in
a lazy process-local LRU keyed by canonical path, modification time and length,
with cache storage bounded by `MAX_FILE_BYTES`. Cache hits stop at the range end
or retention limit. A metadata change invalidates the count; unchanged metadata
cannot detect an external writer restoring the same timestamp and length.

`tool_output(call_id, start_line, end_line)` reads retained raw ranges on demand,
including after restore while the receipt is retained. Cache entries share a
4 MiB `MAX_RETAINED_OUTPUT_BYTES` cap, estimated from ID and raw-output lengths; insertion
evicts the oldest entries, and an individually oversized receipt is not cached.
Unknown or evicted IDs return an explicit reread-file/rerun-tool hint, without
unbounded eviction tombstones. New verbatim cache entries keep the result envelope and
its source metadata; old plain-string entries remain readable. For file receipts,
ranges use original file coordinates within the retained read, preserving the
existing prefixes and line terminators without adding another number prefix.
Ordinary shell receipt reads keep their existing JSON formatting and line-number
behavior. Typed DAG references, JSON work inventories and `_ax_observe` consume
the original source value through `ToolResult::source_text()`: it removes the
exact declared sequence of display prefixes on a separate copy. Stored and
model-visible bodies retain those prefixes and unchanged source bytes.

Nonzero shell exits fail. ACP live updates and replay retain failure status,
result envelopes, original call IDs and tool names. ACP `turn_changes` lists
changed files by comparing file contents before and after the current execution,
including creations, deletions, clean tracked files and ordinary non-Git files.
It persists unified diffs in existing history, never stages/reverts files and is
excluded from model context. Pre-existing edits are the baseline, so untouched
user changes are excluded. An empty final snapshot clears incremental tool edits.
Binary/non-UTF-8 changes are marked explicitly without invented text counts.
Git supplies minimal text hunks; without Git the fallback is a valid full-file
replacement diff. Git ignore rules and runtime/build folders (`.git`, `.ax`,
`AX_HOME`, `target`, `node_modules`, `__pycache__`) are excluded. Snapshots measure
content changes during the run, so concurrent external edits within that interval
can also appear. A failed snapshot is reported, not presented as no changes.
`AX_EVENT_LOG` optionally
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

Search binds the selected actual model adapter through the ordinary kernel and
child composition. `NativeFirst` calls official server search when its concrete
protocol is eligible; unsupported/rejected/incomplete/timeout requests fall back
to `SearchRouter` and anonymous Exa/Parallel MCP. Optional user-key Brave/Bocha
remain available. Native model-account usage can incur normal search/inference
charges; anonymous Remote requires no extra search key. Gateway compatibility
and model names never imply native support. See the exhaustive
[provider audit](web-search-audit.md) and [architecture](web-search-architecture.md).

Queries run concurrently; duplicate input queries are removed. `limit` defaults
to the total target of distinct valid URLs, while `target_results` can increase
that target up to `limit * query_count`. Completed results merge in query order;
remaining requests are dropped when enough results arrive. `cancelled_queries`
identifies unfinished work. A partial failure retains useful results and reports
`succeeded`, `failed` and `errors`; all query failures produce an accurate error.

Canonical URL deduplication drops fragments/tracking parameters while retaining
all matched queries, citations and alternative backend provenance. Records keep
`title`, `url`, `snippet`, `source`, `matched_queries`, and add `evidence` for raw
platform references, grounding/usage and source metadata. The response marks
`content_trust: untrusted_external_data`; retrieved instructions do not authorize
tool execution. Domain-restricted server search fails closed before network calls
because remote visited sites cannot be enforced by local domain policy.

Native probe/health is bound to the account/model; initial failures are coalesced
across a query batch. Search has a 30-second task budget, native attempt at most
12 seconds or half the task budget, and remote 5/10/15-second connect/read/total
limits. Independent remote circuits cool down after three errors; 429 immediately
honors Retry-After. Reaching a result target drops outstanding futures. There are
at most two hedged remote attempts per query and no transport replay retries.
Schema/handshake sessions and health are shared across calls, clones and children;
expired MCP sessions reinitialize on the next task without replaying a search.
Configuration/client replacement resets remote state. Proxy/TLS uses reqwest's
normal behavior; search redirects are rejected and response bodies are bounded.

Positive millisecond environment settings are
`AX_SEARCH_NATIVE_TIMEOUT_MS`, `AX_SEARCH_TASK_TIMEOUT_MS`, and
`AX_SEARCH_<EXA|PARALLEL|BRAVE|BOCHA>_{CONNECT_TIMEOUT_MS,READ_TIMEOUT_MS,TIMEOUT_MS}`.
Remote endpoints use `AX_SEARCH_EXA_URL` and `AX_SEARCH_PARALLEL_URL`. Embedders
retain `with_search_provider`, `with_client`, and `SearchConfig`; endpoint `None`
disables that remote candidate. A lazy shared router preserves reachability and
EWMA latency statistics; `SearchRouter::stats()` exposes them. No page-engine
scraping, DOM search parsing or local engine fallbacks remain.

Fetch remains separately scheduled, with at most three requests in flight.
`target_pages` can return early, reporting `cancelled_urls`; completed pages keep
original URL order and character budgets. Local reads/retries stop on cancellation,
but work already received by a server cannot be undone.

On search failure, the tool description, error and bundled web-research skill
instruct the agent to report/configure/retry rather than automatically search via
shell + Python urllib, curl, or another engine scraper. This is model-facing
routing guidance, not a global shell ban. Explicit user requests for alternative
search methods or network diagnostics are allowed, subject to applicable tool
permissions. `web fetch` remains the known-URL HTTP content retrieval operation.

Fetch retains its shared pooled client, Keep-Alive, 30-second TCP keepalive and
bounded 60-second coalescing DNS cache. Its per-attempt connect/read/total
timeouts remain 2/3/8 seconds, with one 100 ms retry only for timeout/429/5xx;
401/403/404 are not retried. Injected clients retain their transport settings.
Search clients share these pooling/DNS mechanisms but not the aggressive fetch
timeouts or retry policy. Proxy/TLS behavior remains reqwest's defaults.

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
| Built-in tools | `crates/tool/src/shell.rs`, `filesystem.rs`, `patch.rs`, `find.rs`, `search.rs`, `web.rs`, `view_image.rs` |
| Shared traversal/glob policy for discovery | `crates/tool/src/discovery.rs` |
| Workspace path binding for one shared registry | `crates/tool/src/workspace.rs`, `crates/tool/src/sandboxed.rs` |
| `Capability`, `PermissionStore` | `crates/tool/src/permission.rs` |
| Telemetry | `crates/tool/src/telemetry.rs` |
| Runtime scheduling, resource locks, same-round reuse | `crates/core/src/scheduler.rs`, `crates/tool/src/resources.rs` |
| Registry assembly, approval wiring | `crates/cli/src/runtime/builder.rs`, `crates/cli/src/bootstrap.rs` |
| Control tools (`task_queue`, `child_result`, `request_user_input`) | `crates/core/src/task_queue.rs`, `crates/core/src/child_result.rs`, `crates/core/src/user_input.rs` |
| Parallel child dispatch and receipts | `crates/core/src/child_dispatch.rs`, `crates/core/src/child.rs` |
| Project instruction resolution | `crates/core/src/instructions.rs`, `crates/cli/src/project_instructions.rs` |
| `/permissions`, `/tools` views | `crates/cli/src/tui/commands.rs`, `crates/cli/src/tui/commands/catalogs.rs` |

## Shell and child runtime awareness

The shell tool description and command schema expose the actual platform/shell.
Windows uses `powershell.exe` (Windows PowerShell 5.1), not Bash or PowerShell 7.
Commands with unquoted Bash heredoc `<<`, `&&` or `||` are rejected before process
creation with PowerShell recovery guidance. Quoted string/Python contents are
preserved; use PowerShell here-strings piped to Python or separate calls.
Unix uses POSIX `sh`. The description is dynamic tool metadata, not a new system
prompt.

For isolated children, `Tool::fork_for_run` must rebind extension state to the
provided `RunContext`. `SandboxedTool::fork_for_run` delegates to the wrapped
tool's `fork_for_run` and re-applies the boundary, so a child receives the same
tools rebound to `child.cwd` rather than a separate implementation; the
discovery tools carry the workspace they are bound to and `filesystem`
read/list/write are rebound through `WorkspaceTool`. Built-in workspace tools
reject file paths outside that child root, including symlink escapes, and
resolve resource leases to the bound paths. Shell inherits installed
runtimes/PATH but launches with the child cwd and child AX_HOME/session/memory
environment. AX_HOME and memory databases bind to `RunContext.state_dir`,
outside the disposable workspace. Tool execution keeps ordinary approval rules
and the existing DAG. This does not introduce an OS container.


## Execution scope and advisory recovery

Ordinary tool arguments may include `_ax_execution` with `goal_id`, `step`,
`expected_output` and `scope`. The scheduler resolves result references first;
the kernel validates the binding and declared path resources before execution,
then strips runtime-only metadata. New steps may select directories within the
initial workspace. Step subscopes are advisory; the workspace boundary itself is
always enforced, and scope expansion beyond that workspace is rejected. `_ax_observe` optionally names a JSON pointer to an
actual true boolean verifying output; repeated evidence does not reset stagnation.
Every completed result is an observation, including empty/no-match and failure.

Progress, recovery, retries and replanning are orchestration policy. Stagnation
and repeated failures produce advice, never execution rejection. A failed
operation can retry; search/read/list/shell/MCP can diagnose or recover; a model
can switch steps or finish a failed task and continue independent subtasks.
Recovery metadata identifies the failed step, tool call and declared resources;
it does not narrow the declared scope or propagate a global tool lock. Old
checkpoints with failure-derived narrowed scopes restore their original scope
before admission. Permission, sandbox, resource leases, dependencies, configured
budgets/timeouts and cancellation remain independent hard constraints.

Tool-owned runtime storage is explicitly declared through
`runtime_owned_resources`; memory uses it for its private session database.
This declaration covers owned storage, not caller-selected paths. Embedders can
choose an initial workspace with `AgentKernel::with_execution_scope` and must
bind relative tool paths to that workspace. Unknown/global tool effects still
use an exclusive resource lease and their declared permission/sandbox boundary.

Embedded non-harness recursive searches respect the current declared scope. Broader searches can
explicitly bind a new step within the workspace. `search` and
`find_files`/`glob` declare the resolved scope as a read resource, so the same
scope check rejects a call that points outside the current step; the legacy
`fallback_reason` argument is still accepted but never grants access and does not
gate execution.
Discovery skips generated, vendored and cache directories, symlinks, binary
files and files larger than 4 MB, and stops at `max_results` (defaults 100 for
`search`, 200 for `find_files`). Limits produce partial/truncated results rather
than errors.


## Subagents

Delegation tools are registered by default when a `ChildHost` is attached.
One delegation implementation is instantiated as two tools; `max_depth = 0`
disables both without initializing named-Agent metadata or provisioning children:

- `subagent(task, context?, tools?, policy?)` — spawns a **fresh-context**
  child: no parent conversation, isolated memory/workspace by default. The
  task must be self-contained. Agent templates narrow the child further.
- `subagent_fork(prompt, description?)` — delegates to a child **seeded with
  the parent's completed turns** (the current in-flight turn is never
  inherited). It shares the parent workspace and provider, so the inherited
  prefix stays eligible for provider-side cache reuse. The contract is fixed:
  no model selection, no tool narrowing.

The concurrency pool and depth budget are tunable. `/settings` edits global
defaults; `ax settings --max-concurrent 8 --max-depth 2` is the CLI form.
`--scope project` reads or writes overrides in the discovered project's
`.ax/config.toml`; omitted fields inherit the global settings. `--reset` restores
global defaults (8 / 1), or removes the project's two overrides without touching
other configuration. AXCrew exposes these controls under Settings → Plugins →
Agents, using the selected global/project scope. Changes are loaded before the next agent
turn, including in a running TUI/ACP session. The JSON section is:

```json
"subagent": { "max_concurrent": 8, "max_depth": 1 }
```

Legacy `config.toml` sections that still carry an `enabled` key migrate through
the existing config loader; the key is ignored.

The runtime primitives are `spawn_agent`, `wait_agent`, and `cancel_agent` on
the kernel or its optional manager handle. Embedders attach a `ChildHost` and
call `prepare_subagents` before direct primitive use; normal agent turns
prepare the handle automatically.

Independent calls use the existing tool-round DAG and bounded concurrency.
Ordinary subagents and queue children at all levels share `max_concurrent`
(default 8, 1–64); provisioning and ancestors
waiting for their descendants count toward this bound. Root tasks can queue;
nested tasks reject exhausted capacity instead of waiting on an occupied ancestor.
At most 64 ordinary subagents are admitted across the turn's delegation tree;
queue inventories may contain more than 64 items and reuse the shared slots.
`max_depth` accepts non-negative integers: 0 disables delegation, 1 permits direct
children, and larger values permit further delegation up to that absolute depth.
At the cap, both delegation tools are absent and queue child dispatch is rejected.
The main model judges whether focused context, checking or parallel work justifies
delegation across research, writing, analysis, planning and software tasks. No
fixed profession or complexity threshold triggers a child. Small or tightly
coupled work can stay with the main agent; user requests to work alone or in
sequence take precedence. Changing the limit does not spawn children or stop an
already running turn.
Tools are a whitelist intersected with the child's rebound registry; unknown
tools and delegation requests beyond the depth budget are rejected. Empty tools
disables all child tools, including delegation. Provider/model/reasoning and the parent approval
policy are inherited; tools and histories are not shared across child sessions.
Every delegated child receives the delegation contract as context: its
permission scope was fixed at start, denied operations must not be retried, and
limitations belong in the reply to the delegating agent.

Each tool returns `{status, summary, artifacts, error}`. Status is `completed`,
`failed` or `cancelled`; artifacts identify the durable child state directory,
when supplied by the host. Disposable workspace changes are not merged into the
parent. Only the final receipt is returned; raw history remains in the child
store. Runtime events are `subagent_started`, `subagent_progress`,
`subagent_completed`, `subagent_failed` and `subagent_cancelled`. TUI renders
brief lifecycle notices without child text, tool inputs or reasoning.
Cancellation and timeouts stop the existing child loop, cancel descendants and
close their receipts. Shared-workspace children acquire resource leases through
their own effect tools; the delegating call never holds a lease while awaiting them.
Primitive timeouts include admission/provisioning; model-tool children
inherit the configured child timeout. Dropping a parent tool round cancels its
outstanding delegated calls. Provider, provisioning, receipt and worker failures
are returned locally in the result envelope.

## Permission profiles and sandbox boundaries

The runtime evaluates `PermissionProfile` rules on resolved tool arguments and
declared resources before executing a tool, including after result references
are resolved. `RuleMatcher` supports command globs, command prefixes,
normalized filesystem path globs, domain globs and JSON-pointer tool arguments.
Overlapping rules resolve **deny > ask > allow**, independent of order.
Explicit capability Deny remains a ceiling. Explicit Ask requires an interactive
approval even when a capability has a session Allow; approving it never grants
other calls. Noninteractive explicit Ask fails closed. Rules are enforced in
Runtime/transport, never through model instructions.

```json
"permissions": {
  "rules": [
    {"decision":"allow", "matcher":{"kind":"command","pattern":"cargo *"}},
    {"decision":"ask", "matcher":{"kind":"prefix","value":"git push"}},
    {"decision":"deny", "matcher":{"kind":"path","pattern":"~/.ssh/**"}},
    {"decision":"deny", "matcher":{"kind":"path","pattern":"/etc/**"}},
    {"decision":"deny", "matcher":{"kind":"domain","pattern":"*.internal.example"}},
    {"decision":"ask", "matcher":{"kind":"tool_parameter","tool":"mcp*",
       "pointer":"/arguments/action","pattern":"delete"}}
  ],
  "boundary": {"read_only":false, "deny_network":false}
}
```

Paths expand `~`, resolve aliases/existing symlinks and normalize missing
components. Parent-tree/unknown resource access cannot bypass a denied subtree.
Command Allow never authorizes compound shell/substitution syntax without Ask.
Arbitrary shell, Process and MCP effects cannot be determined from arguments:
restrictive path/domain rules conservatively reject or ask for those calls.
The OS sandbox remains an additional, independently enforced boundary; a rule
cannot relax it or select an unavailable backend. Web fetch checks redirect
hops; a new denied/Ask host stops the redirect. Search/unknown network
transports fail closed when domain rules cannot be enforced. This conservative
fallback keeps policy enforcement local without adding a privileged proxy.

## ChildPolicy inheritance

The same `ChildPolicy` controls every explicit subagent invocation:

```yaml
context: none          # none | summary | last_n | full
last_n: 0             # positive complete-turn count when context=last_n
memory: isolated      # none | parent_readonly | isolated | shared_project
skills: none          # none | selected | inherit
selected_skills: []
tools: inherit        # none | selected | inherit; only child-bindable parent tools
selected_tools: []
mcp: none             # none | selected | inherit
selected_mcp: []
model: inherit        # inherit | override
model_override: null  # parent-registered key
workspace: isolated   # shared | snapshot | isolated
permissions: inherit_restricted # inherit_restricted | custom
custom_permissions: { rules: [], boundary: {} }
```

Defaults preserve legacy safe isolation and no recursive delegation. Legacy
`context` text is explicit task input and `tools` is an additional narrowing
whitelist. Parent history is readonly context, with queue/execution state
removed; last_n keeps complete turns. Memory `none` removes memory access,
`parent_readonly` rejects mutation, and `shared_project` permits only the
parent project store, with child input provenance. Isolated memory retains its
own session/database. Selected/inherited skills retain the parent's eligible
catalog and lazily load only selected bodies. Unbound extensions are never
silently inherited.

LocalChildHost maps snapshot and isolated to its existing filtered workspace
provisioner (Git worktree plus dirty inputs, otherwise filtered copy). Shared
binds tools to the parent's workspace while retaining separate durable child
state and a disposable lifecycle workspace; cleanup never deletes the shared
parent workspace. A host that cannot implement a requested binding rejects it.
Parent MCP inheritance requires explicit shared workspace because existing
server resources are parent-scoped. All children retain the parent's effective
profiles and capability approval object; custom rules only add restrictions.

Model overrides must be configured by the parent. Embedders call
`register_child_model`; CLI uses the optional local `child_models` map:

```json
"child_models": { "reviewer": { "provider":"openai", "model":"gpt-4.1" } }
```

Building an approved provider performs no model/network request. An unknown
model key is rejected; the child cannot create credentials or providers.

## Explicit task queue planning

Formatting alone never creates tasks. `TaskQueue::list_hints()` is optional
metadata for a caller/model; it cannot schedule or spawn anything. The main
model explicitly calls `task_queue(action="start", overall_goal, tasks)`.
`execution="controller"` is the default, including new typed `ax_work_items`
inventories; only explicit `execution="children"` delegates tasks through
ChildHost. `action="configure"` requires `execution` and changes the current
active queue's mode while preserving task IDs, dependencies, progress and receipts.
Appending typed records preserves an existing explicit children choice when the
current runtime permits it. `dependencies` contains zero-based prior
task indices, one list per task. Invalid/forward dependencies are rejected;
a failed dependency skips dependents while independent tasks continue.
Queue responsibilities are state, dependencies, scheduling and outcomes.
Old serialized queues retain their state; existing child receipts allow an
explicit goal resume to continue an already delegated run within current limits.
Depth 0 or inherited empty delegation tools prevent both new and restored child
dispatch. A restored prohibited queue retains its inventory/child handles and
returns to controller execution with feedback, so it can continue directly.

`resources` is optional per task: a bare string is a read of that path, and an
object carries `path` or `name` plus `write`. The runtime refuses to run two
tasks whose declared accesses conflict, so a shared file must be declared rather
than assumed safe. All ready, non-conflicting tasks are dispatched in one batch
and newly ready tasks are admitted as children settle; concurrency shares the
ordinary subagent pool (default 8), never a separate prompt instruction or queue
allowance. `with_child_concurrency` sets that same limit for embedders.
Queue delegation uses the ordinary tools' declared Process/Safe permission
boundary, including profile Deny/Ask and saved capability decisions. The current
controller choice is authorized once before dispatch, not once per item; a new
host turn rechecks authorization. Availability context is not a permission grant.

## Control tools and structured receipts

Three tools are handled by the kernel rather than by a registry entry, because
they change orchestration state instead of touching the world:

| Tool | Purpose |
|---|---|
| `task_queue` | Create/append tasks, configure execution, finish, block or cancel a goal's tasks. |
| `child_result` | Read a full child receipt by `child_id` (`full`, `findings`, `diagnostics`, `artifacts`, `diff`, `validation`, `metrics`); omit `child_id` to list receipts. |
| `request_user_input` | Ask the user one structured question (`question`, `options`, `allow_free_text`) when the answer materially changes the result. |

Each must be called alone in a round; a mixed round is rejected and nothing in it
executes. They are offered to the main agent only: a child owns no queue, cannot
park the controller's run, and cannot orchestrate.

`request_user_input` is not a permission request. Dangerous operations are
authorized by the permission system, and a question never grants or denies
anything. Asking checkpoints the goal, queue, session and question, parks the
queue in `waiting_for_user` and suspends the run; the answer is written back to
the same tool call id, so execution resumes in place with no new user turn.
Frontends surface it through `AgentEvent::UserQuestion`: the TUI shows it and
treats the next submission as the answer, ACP sends an `agent_question` session
update, and a non-interactive `ax run` reads the answer from stdin.

A child's receipt is a `ChildResult`. The controller model receives only
`model_summary()`; the full record is durable and is read through `child_result`,
so recovering detail never means re-running a child. A receipt recovered from an
earlier process short-circuits provisioning, so a resumed session never repeats a
completed child. Metrics (wall time, model rounds, tool calls, reported tokens,
failed tool calls) come from the child's own event stream and transcript.

## Sandbox enforcement

Permission expresses AX's willingness to request an action. Workspace Sandbox
is the separate OS boundary underneath it. Workspace workers run normal development
operations without repeated approval; explicit denials still apply. That reduced
approval is decided by the confined policy actually bound to the executing tool,
never by the process-wide configured mode, and an unprepared or unconfined binding
keeps the tool's own approval requirement. An escape returns
SandboxViolation and never triggers an automatic host retry. New local-effect tools
must declare a workspace worker boundary and use SandboxManager, rather than adding
path-string checks or host spawn paths. See [security.md](security.md).

The runtime treats model-declared step subscopes as advisory while enforcing
the initial workspace boundary. This prevents a guessed nested scope from
locking recovery. See [coding-harness.md](coding-harness.md).

## SSH execution context

AX_SSH_CONTEXT_FILE points to a JSON manifest supplied by AXCrew to local AX.
This avoids command-line/environment length limits for large SSH catalogues.
AX_SSH_CONTEXT can alternatively contain the JSON directly. Both formats carry
hosts (id, name, host, optional port/identity_file), default_host and remote cwd.
Malformed contexts fail rather than falling back to local file execution.
The remote runtime exposes ssh and web instead of local shell/filesystem/patch
tools. Model credentials and inference remain local; no remote AX is required.

ssh action=list returns host IDs/names/endpoints without key-file paths.
ssh action=exec takes command, optional host_id/cwd/timeout_seconds. The selected
host/cwd are defaults; another host defaults to its login directory. Commands
travel as stdin to a fixed OpenSSH remote sh process, never through a local
shell. Result contains host_id, cwd, exit_code, stdout and stderr. Commands require
Shell approval; read-only/no-network profiles deny them, and domain-rule
profiles fail closed because SSH cannot enforce URL-level filtering.

There is no configured SSH host count limit or independent-host tool pool cap.
Per-host resources serialize conflicting effects in one turn; separate hosts
can run concurrently. Timeouts kill the local SSH process; remote process cleanup
still depends on the remote SSH server/shell's disconnect handling.

## Optional Long Project tool

An explicitly enabled Goal registers runtime-owned `long_project`: evidence-linked
section notes, current document/revision, archive lookup and paged raw tool receipt
retrieval scoped to the same Goal's historical Sessions. Scheduler resource leases
and declared filesystem read/write capabilities still apply. Notes cannot admit
completion or replace Task state; credentials and unverified source IDs are
rejected. Children do not inherit this host tool. Ordinary tasks register nothing.
See [Long Project](long-project.md) for parameters and conflict handling.
