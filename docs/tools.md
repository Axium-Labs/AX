# Tools, Permissions & Safety

This document covers AX's built-in tools, how tools declare their permission
requirements, and how the runtime and UI enforce them.

## Agent environment and Crew terminal shell

`ax environment` shows execution settings. On Windows, `ax environment native`
runs AX directly; `ax environment wsl` runs Linux AX in the default WSL
distribution. WSL selection first checks `~/.local/bin/ax --version`; install
the Linux AX binary there before selecting WSL. `/environment` opens the same
settings in the TUI. Changes take effect on the next AX launch.

The Windows launcher forwards stdio, exit status, the workspace and path flags
to Linux AX. It maps the shared `AX_HOME` and ACP session `cwd` through
`wslpath`. Agent shell commands use the Linux runtime's shell in WSL.

`ax environment --terminal-shell powershell|cmd|git-bash|wsl` independently
selects the shell for new Crew integrated terminals. Crew exposes both settings
under Settings > AX and validates the chosen terminal program before saving.
Existing processes and terminals keep their current environment.

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

The registry is assembled per composition root (`runtime::tools` in
`crates/cli/src/runtime/builder.rs`): the built-ins,
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
| Registry assembly, approval wiring | `crates/cli/src/runtime/builder.rs`, `crates/cli/src/bootstrap.rs` |
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
provided `RunContext`. Built-in workspace tools reject file paths outside that
child root, including symlink escapes, and resolve resource leases to the bound
paths. Shell inherits installed runtimes/PATH but launches with the child cwd
and child AX_HOME/session/memory environment. AX_HOME and memory databases bind
to `RunContext.state_dir`, outside the disposable workspace. Tool execution keeps ordinary
approval rules and the existing DAG. This does not introduce an OS container.


## Execution scope and advisory recovery

Ordinary tool arguments may include `_ax_execution` with `goal_id`, `step`,
`expected_output` and `scope`. The scheduler resolves result references first;
the kernel validates the binding and declared path resources before execution,
then strips runtime-only metadata. New steps may select directories within the
initial workspace; the same step may narrow its scope. Scope expansion beyond
that workspace is rejected. `_ax_observe` optionally names a JSON pointer to an
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

Recursive searches respect the current declared scope. Broader searches can
explicitly bind a new step within the workspace. `fallback_reason` is an optional
orchestration note and never grants access outside the resource scope.
Search traverses
at most 2,000 entries, eight directory levels and two seconds; skips symlinks,
binary files and files larger than 1 MB; and excludes `.git`, build/target/dist,
node_modules, child-runs, caches, virtual environments, runtime storage and vendor
outputs. Limits produce partial/truncated results or an explicit timeout error.


## Optional subagents

Subagents are disabled by default. `/settings` toggles delegation and persists it
in the existing AX config; `/settings subagent on` and `/settings subagent off`
are also supported. CLI settings use `ax settings --subagent true|false`,
`--max-concurrent 3`, and `--max-depth 1`. Changes are loaded before the next
agent turn, including in a running TUI/ACP session. The JSON section is:

```json
"subagent": { "enabled": false, "max_concurrent": 3, "max_depth": 1 }
```

Legacy `config.toml` supports the equivalent section and migrates through the
existing config loader:

```ini
[subagent]
enabled = false
max_concurrent = 3
max_depth = 1
```

When disabled, no delegation tool, manager, child provisioning, extra prompt,
planner request or model call is created. Enabling adds only
`subagent(task, context?, tools?, policy?)`; the model decides whether to use it. The
runtime primitives are `spawn_agent`, `wait_agent`, and `cancel_agent` on the
kernel or its optional manager handle. Embedders attach a `ChildHost`, configure
subagents and call `prepare_subagents` before direct primitive use; normal agent
turns prepare the handle automatically.

Independent calls use the existing tool-round DAG and bounded concurrency.
Delegation admission also limits children to `max_concurrent` (default 3,
1–64); provisioning counts toward this bound. At most 64 tasks are admitted per
turn, preventing unlimited spawn even with an unlimited execution budget.
`max_depth` is 0 or 1; all children have delegation removed regardless of
configuration. Tools are a whitelist intersected with the child's rebound
registry; unknown or delegation tool requests are rejected. Empty tools disables
all child tools. Provider/model/reasoning and the parent approval policy are
inherited; tools and histories are not shared across child sessions.

Each tool returns `{status, summary, artifacts, error}`. Status is `completed`,
`failed` or `cancelled`; artifacts identify the durable child state directory,
when supplied by the host. Disposable workspace changes are not merged into the
parent. Only the final receipt is returned; raw history remains in the child
store. Runtime events are `subagent_started`, `subagent_progress`,
`subagent_completed`, `subagent_failed` and `subagent_cancelled`. TUI renders
brief lifecycle notices without child text, tool inputs or reasoning.
Cancellation and timeouts stop the existing child loop and close the child
receipt. Primitive timeouts include admission/provisioning; model-tool children
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
`execution="controller"` is the default; only explicit `execution="children"`
delegates tasks through ChildHost. `dependencies` contains zero-based prior
task indices, one list per task. Invalid/forward dependencies are rejected;
a failed dependency skips dependents while independent tasks continue.
Queue responsibilities are state, dependencies, scheduling and outcomes.
Old serialized queues retain their state; existing child receipts allow an
explicit goal resume to continue an already delegated run.

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
