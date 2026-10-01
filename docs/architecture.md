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
                   ├──> skill ────┐
                   ├──> memory ───┴──> lexical
                   └──> mcp ───────────> tool (proxy implementation)
```

## Crate responsibilities

| Crate | Responsibility |
|---|---|
| `model` | Provider-neutral `ModelProvider`, text/image message parts, capabilities, tool calls, response types. One streaming entry point (`complete_stream`) with a non-streaming fallback. |
| `tool` | `Tool`, `ToolRegistry`, JSON Schema, `SafetyLevel`, plus built-ins: `shell`, `filesystem`, `patch`, `search`, `web`, `view_image`. Owns the `PermissionStore`. |
| `runtime-core` | The model → tool → model agent loop, `AgentEvent` stream, context selection, `ContextBudget`, compaction, and `AgentSupervisor` for bounded-concurrency tasks. |
| `mcp` | MCP client for stdio / Streamable HTTP / WebSocket, lazy connection, capability catalog, `McpToolProxy` and `McpGateway`. |
| `skill` | `SKILL.md` frontmatter indexing, precomputed Unicode routing features, and language-independent similarity routing; Markdown body is loaded only when a route hits. Legacy packages remain supported. |
| `scoped` | Shared Global/Project registry, config policies and override/mask resolution. Metadata only, no execution or UI dependencies. |
| `lexical` | Language-independent lexical features (NFKC, case fold, word tokens, character n-grams) and normalized similarity, shared by skill routing and memory retrieval so the two cannot drift apart. Leaf crate: no dependency on other AX crates. |
| `memory` | SQLite session/message repository, effective-context snapshots, scoped facts, JSONL event streams, resume and compaction state. |
| `cli` | The only composition root: clap arguments, provider selection, lazy SQLite/Skill/MCP initialization, REPL, ratatui TUI, session commands, permission dialogs. |
| `evolution` | CLI-fed bounded Experience observation, low-frequency in-process analysis, owned standard Skill lifecycle and learned Project facts; no core/tool dependency. |

The explicit `ax --update` path belongs to the CLI and exits before normal
runtime setup. It checks the latest GitHub Release, verifies its archive against
`SHA256SUMS`, and replaces the current executable. On Windows replacement is
deferred until the running process exits. It never touches user or project data.

### `model`

Defines the provider-neutral `ModelProvider` trait, message types, tool calls
and responses. `complete_stream` is the unified streaming entry point and
falls back to a non-streaming implementation by default. Existing protocol
adapters:

- OpenAI-compatible Chat Completions (DeepSeek, Groq, Mistral, OpenRouter,
  Together, xAI, Kimi and other compatible providers);
- Native WorkBuddy browser authorization, rotating OAuth credentials and direct
  Chat Completions transport (see [providers.md](providers.md#native-workbuddy));
- OpenAI Responses API;
- Codex local-file auth + ChatGPT Codex Responses endpoint.

`ProviderSpec` holds vendor identity, credential environment variable and
protocol selection. `OpenAiCompatibleProvider` implements the shared Chat
Completions request and response format; DeepSeek's endpoint and defaults stay
in provider metadata.

Providers also report their context window, which drives the kernel's
compression policy. Adding a local model means implementing the trait — no
agent-loop changes.

Credentials are persisted in `<install-dir>/.ax/auth.json` (or `AX_HOME`): on Unix the file is written
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

- The **agent loop** with an `ExecutionBudget` (max model steps, max
  tool calls, per-turn timeout, per-tool timeout). All four limits are
  unlimited by default; positive CLI flag values enable finite limits.
  When a configured budget or timeout fires, hanging tool calls get a
  placeholder result so the message record stays well-formed.
- The **`AgentEvent` stream**: token deltas, tool state, compression events.
- **Dependency-aware tool rounds**: independent declared effects execute with
  bounded concurrency; typed result references and shared write resources add
  DAG edges. Process-wide leases serialize conflicting filesystem, Git and
  Memory operations across kernels. Lifecycle events and results retain the
  real tool call IDs. Raw checkpoints follow completion order; the next model
  receives the completed round in call order. See [tools.md](tools.md#concurrency)
  and [ADR 0005](adr/0005-tool-round-scheduler.md).
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

The core has no fixed system prompt. Compaction uses a focused, impersonal
summarization instruction; long-task turns add only transient progress and
recovery state.

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

On first use, indexes only standard `SKILL.md` YAML frontmatter (or a legacy
`skill.toml`), and derives each skill's Unicode routing features once at index
time. Routing compares the task text with the name and description by
normalized lexical similarity (words plus character n-grams, from the shared
`lexical` crate), with no language
detection, stemming or stopwords, so every script ranks through one code path.
After activation, the selected Markdown body is read and injected within the
`ContextBudget` skill reserve; optional resources remain on demand. Project
skills take precedence over global skills and malformed packages are isolated.
A compact metadata-only catalog lets the model select a skill by meaning through
the ordinary filesystem tool when strong automatic matching misses it; AX omits
that catalog when automatic routing has already selected a skill.
`allowed-tools` never changes AX tool permissions. See [skills.md](skills.md).

### `memory`

SQLite stores raw session messages, effective-context snapshots and scoped
facts. Global facts live in the AX home; Project and Session facts live in the
selected installation-owned project database. Project ownership is a UUID
persisted in `<install-dir>/.ax/projects/<project-key>/project.json`, independent
of `--data-dir`; legacy workspace IDs remain readable. Runtime state survives
workspace deletion. See [storage.md](storage.md) for migration and path keys.

Explicit `remember key=value` declarations default to Session scope. Natural
language intent is handled by the main model through a session-bound `memory`
tool; code validates scope ownership, quoted user provenance, credential
patterns, and optimistic-update preconditions. Retrieval ranks facts by a
composite of the shared `lexical` similarity (computed once per turn for the
query, precomputed and cached per fact), scope priority, recency and
importance, then applies the shared token budget — with no per-language rules.

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
3. The skill instruction body is read only when a route hits.
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
      → goal-bound step/scope admission
      → permission check
      → dependency DAG and bounded resource-aware execution
      → update ExecutionState from actual completion events
      → checkpoint completed results and execution state with original tool call IDs
      → append completed round in original call order
      → context pressure check / next model step
```

The loop has no default step, tool-call, turn-time or tool-time limits.
`--max-steps`, `--max-tool-calls`, `--turn-timeout-secs` and
`--tool-timeout-secs` accept positive limits; `0` means unlimited.
When a configured budget or timeout fires, any hanging tool
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
| A new skill | Add a `SKILL.md` package |
| A new UI | Consume `AgentEvent` and call the kernel |
| New storage | Keep session/message repository semantics; don't leak the database into core |
| Portable backup | Use the memory crate ExportService/ImportService; CLI handles arguments and display |

Step-by-step guidance lives in [development.md](development.md).

## Lightweight long-task orchestration

The existing Agent Loop and tool-round DAG remain the execution mechanisms. A
small Task Queue recognizes top-level numbered/bulleted multi-task requests;
the execution model can also initialize semantic subtasks through `task_queue`.
There is no separate planner call or fixed orchestration system prompt.

Queues belong to a `goal_id`, not a session. Ordinary user turns start a new goal,
supersede and archive any resumable old queue, and persist a fresh state head.
`GoalTurn::Resume` restores only the explicitly requested saved goal ID. Budget
interruptions suspend that goal for explicit resume. CLI exposes `ax run --session
<ID> --resume-goal <GOAL_ID> <PROMPT>` and `--cancel-goal`; ACP uses prompt metadata
`_meta.axGoal` with `action` new/start/resume/cancel and `goal_id` where applicable.
Successful ACP prompt responses return the goal ID in `_meta.axGoal.goal_id`.

Tasks have pending/running/completed/failed/skipped states. Explicit `finish`
controls advance the queue; task failures require a recovery opportunity first.
Tool failures and tool timeouts remain local, allowing independent tasks to
continue. Recovery is bound to the failed execution step and its task directories.
Unbounded recovery tools are rejected; successful retries restore the original
step scope, and repeated failures require a different bounded strategy. Provider failures retry once within the goal; configuration,
authentication, persistence and exhausted provider retries block the goal.

Queue states are active/summarizing/suspended/completed/blocked/cancelled/superseded.
Explicit global `block`/`cancel` controls and typed `ToolError::GlobalBlocked`
stop queue consumption immediately. A text-only response never advances a task:
with unfinished tasks it terminates the goal as blocked; after terminal tasks it
completes the summary. The terminal response is checkpointed before output and
cached for reconnect without another model call or repeated response events.
Worker forks strip controller queue state and retain only parent goal identity,
so isolated worker contexts do not require manually created user sessions.

Queue checkpoints use the existing AgentState path; see [context.md](context.md),
[storage.md](storage.md) and [ADR 0008](adr/0008-durable-task-queue.md).

### Automatic isolated child execution

The CLI attaches a `ChildHost` to the kernel. Once a queue exists, the controller
consumes its running/pending entries automatically, calls `AgentSupervisor::run_child`
and collects each outcome. Children use the existing kernel loop; they do not
receive the controller queue or siblings' messages. The controller makes a single
text-only summary request after all children are terminal. Plain single-task
turns keep their existing loop. Embedders can opt in via `with_child_host`.

`LocalChildHost` separates `child-runs/<id>/state` (session database, JSONL,
terminal receipt and lifecycle manifest) from the disposable `workspace` directory.
Git projects use detached worktrees at the captured HEAD plus a binary dirty patch
covering staged/unstaged tracked changes, renames and deletions. Only eligible
untracked inputs are copied; unchanged tracked files are never recursively copied
from the controller. Non-Git projects or unavailable Git use filtered snapshots.
Nested `.gitignore` and `.axignore` rules, including negation, exclude inputs;
controller stores, sibling history, symlinks and build/cache directories are excluded.
An existing Git repository with worktree/patch errors fails that child instead of
silently copying the entire project. Runtime/scripts and installed PATH remain available.

Completed/failed children persist their receipt before removing the workspace and
Git worktree registration. Cleanup failures are retried by GC; terminal receipt
recovery works without a workspace. Running/interrupted children retain their
workspace for resume. Legacy `workspace/.ax` stores migrate into `state` on resume.
File leases prevent GC from touching active children, and serialize provisioning
across processes sharing a storage root. GC runs on child admission, removes
terminal workspaces and expires unleased workspaces after the configured idle TTL;
it never deletes raw session history. Expired children fail explicitly on resume
rather than silently restarting. Workspace changes are disposable, are not merged
back, and are not a retained artifact; durable outputs/history are in `state`.

Child tools opt in through `Tool::fork_for_run(RunContext)`. File/image/search/patch
tools bind relative paths and resource leases to the child workspace; shell
subprocesses use an explicit cwd and child AX_HOME/session/memory environment.
Memory operations of every scope use the child's own store. Unbound extensions
and parent MCP processes are not silently shared; an extension must supply a
child-scoped binding before it is available. Raw result readers are independent.
Tool event IDs are prefixed with the child session to prevent UI collisions.

Child failure, unresolved tool errors, workspace failure and child timeout record
only that child's failure and advance the queue. Embedders can set independent
limits through `with_child_execution_budget`; the CLI exposes `--child-timeout-secs`
separately from the controller `--turn-timeout-secs`. Controller ExecutionBudget remains
a global stop/suspend boundary. Controller cancellation/blocking stops consumption.
Raw child history and a terminal receipt are persisted in the child's session.
Resume reuses the saved child identity/cwd/history; terminal receipts prevent
replaying a completed child if the controller checkpoint was interrupted.
Unknown interrupted tool outcomes get placeholders for inspection, not automatic
side-effect replay. Child final text is withheld from controller final output.

Workspace quotas default to 2 GiB per child and 8 GiB per child-storage root;
idle workspace TTL defaults to 7 days. Positive integer environment overrides:
`AX_CHILD_WORKSPACE_QUOTA_BYTES`, `AX_CHILD_TOTAL_QUOTA_BYTES`,
`AX_CHILD_WORKSPACE_TTL_SECS`. Admission preflights snapshot/Git checkout sizes
and remaining space; each raw checkpoint checks actual workspace usage. Over-quota
execution fails only that child, persists its failure and releases workspace space.
These are admission/checkpoint limits, not an OS filesystem quota: a subprocess
can temporarily exceed them between checkpoints. Durable state is excluded from
workspace quota accounting and is preserved by GC.


### Goal-bound execution invariants

`ExecutionState` lives in the kernel independently of conversation compression.
It retains the original goal and identity, current step, expected output, task
scope, failed-step recovery binding, bounded real completion events and progress.
The execution model can declare a narrower step through `_ax_execution` on an
ordinary tool call. A successor requires observed progress; recovery cannot
replace the failed step or enlarge its directories. Scope admission uses declared
resources after typed result substitution, before approval and execution.

Eight consecutive calls without a declared state transition trigger
`NoProgressDetector`. Read/list/search success alone is observation; new successful
declared mutations, explicit result evidence and repaired retries count as progress.
Repeated mutations/evidence are deduplicated. A stall binds recovery to the current
step, injects its goal and ineffective actions and rejects repeated exploration.
No separate planner/model request is made. See [context.md](context.md),
[tools.md](tools.md) and [ADR 0009](adr/0009-execution-invariants.md).


### Optional model-selected delegation

`SubagentConfig` defaults to disabled. The enabled turn registers a single
`subagent` tool backed by a turn-scoped admission/cancellation adapter. Runtime
`spawn_agent` / `wait_agent` / `cancel_agent` reuse `ChildHost::prepare` and
`AgentSupervisor::run_child`; the latter is the existing checkpointed Agent Loop.
There is no second loop or planner request. The adapter enforces bounded child
admission and suppresses child content/reasoning in parent events, forwarding
only compact lifecycle events. Worker forks remove delegation and controller
history. CLI reloads the existing persisted settings before each turn. See
[tools.md](tools.md#optional-subagents) and [ADR 0010](adr/0010-optional-subagents.md).

## Capability scope composition

Skill, MCP and named Agent metadata resolve through one `scoped::ScopedRegistry<T>`.
The CLI supplies source adapters and a shared manager. Global and project sources
merge by name/ID before explicit project overrides and inherited-global masks;
only enabled entries reach execution. Crew consumes AX's ACP scope rows and
management boundary. Named Agent instructions are read at invocation. Portable
project capability config carries the existing project UUID; runtime stores keep
their existing ownership and migration rules. See [capabilities.md](capabilities.md)
and [ADR 0011](adr/0011-scoped-capabilities.md).
