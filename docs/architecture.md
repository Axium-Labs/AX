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
| `tool` | `Tool`, `ToolRegistry`, JSON Schema, `SafetyLevel`, plus built-ins: `shell`, `filesystem`, `find_files`/`glob`, `patch`, `search`, `web`, `view_image`. Owns the `PermissionStore`. |
| `runtime-core` | The model → tool → model agent loop, `AgentEvent` stream, context selection, `ContextBudget`, compaction, and `AgentSupervisor` for bounded-concurrency tasks. |
| `mcp` | MCP client for stdio / Streamable HTTP / WebSocket, lazy connection, capability catalog, `McpToolProxy` and `McpGateway`. |
| `skill` | `SKILL.md` frontmatter indexing, precomputed Unicode routing features, and language-independent similarity routing; Markdown body is loaded only when a route hits. Legacy packages remain supported. |
| `scoped` | Shared Global/Project registry, config policies and override/mask resolution. Metadata only, no execution or UI dependencies. |
| `sandbox` | Runtime confinement policies, reusable OS execution backends and workspace managers below tools and local MCP transport. |
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
`shell`, `filesystem`, `find_files`/`glob` (name/path/extension discovery),
structured `patch` (multi-hunk edits; any failing hunk aborts the whole write)
and `search` (line-scoped content, symbol and regex search) to reduce shell
abuse. Discovery tools share one traversal and glob policy
(`crates/tool/src/discovery.rs`) so a filename scan and a content scan of the
same root can never disagree.

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

Web search is query concurrency → a lazy reusable `SearchRouter` → independently
bounded Bocha/Brave/SearXNG adapters and the final keyless DuckDuckGo fallback.
The router owns latency/failure history, temporary circuits, bounded hedging and
cancellation; protocol adapters only normalize responses. Search has separate
transport timeout configuration; known-URL fetch keeps its existing semantics.
See [tools.md](tools.md#web).

### `runtime-core`

Internal layout: `kernel/` (state, construction, goal lifecycle),
`loop_runtime/` (one model step, one tool step, the turn loop that orders
them), `compression/` (pipeline and structured summary), `event` / `error` /
`approval` / `token` as the kernel's outward contracts, and `budget` /
`context` / `scheduler` / `execution` / `child` / `child_policy` / `subagent` /
`supervisor` / `task_queue` for execution machinery. `lib.rs` is a façade of
`mod` declarations and `pub use` re-exports only.

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
- **`ContextBudget`** (`runtime-core::ContextBudget`, derived in `crates/core/src/budget.rs`): one structure from which every
  context-space limit is derived — reserved output tokens, tool-schema
  estimate, one elastic context pool, per-source maxima and projected next-request compaction. Modules no longer hard-code their own
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
`skill.toml`) and precomputes lexical routing features. Features only rank
candidates. The main model receives compact eligible metadata and explicitly
calls `invoke_skill`; body/resources stay lazy. Deterministic enabled/scope/
path/dependency/implicit-invocation filters constrain selection. No extra
routing model request is added. See [skills.md](skills.md).

### `memory`

Internal layout: `store.rs` (open, migrations, `MemoryStore`),
`schema.rs` / `migrations.rs` (additive schema and its order), `session.rs` /
`message.rs` / `context.rs` / `long_term.rs` (the four stored concerns),
`events.rs` (the JSONL stream and its index), `types.rs` / `error.rs`, plus
`scoped.rs` and `backup.rs`. `MemoryStore` stays the single façade.

SQLite stores raw session messages, effective-context snapshots and scoped
facts. Global facts live in the AX home; Project and Session facts live in the
selected installation-owned project database. Project ownership is a UUID
persisted in `<install-dir>/.ax/projects/<project-key>/project.json`, independent
of `--data-dir`; legacy workspace IDs remain readable. Runtime state survives
workspace deletion. See [storage.md](storage.md) for migration and path keys.

Explicit `remember key=value` declarations default to Session scope. Natural
language intent is handled by the main model through a session-bound `memory`
tool; code validates scope ownership, quoted user provenance, credential
patterns, and optimistic-update preconditions. Retrieval filters the current Session/Project and Global owners, recalls at
most 32 lexical/exact/tag/path candidates, then lightly reranks by relevance,
usage, confidence and type-aware freshness. Scope is a filter, never a score.
Only a few SQLite index summaries enter the shared context budget; details
are fetched by key on demand. Existing Unicode routing features are preserved.
There are no embeddings, vector stores, per-language rules or extra model calls.

Every complete conversation message is checkpointed before execution
advances. Resume queries pages after the snapshot watermark and marks
interrupted tool calls without replaying their side effects. Compression
snapshots stay session-local; raw messages are retained. See
[memory.md](memory.md) and [storage.md](storage.md).

### `cli`

Internal layout: `main.rs` parses arguments, bootstraps, dispatches and exits;
`args.rs` (the clap definition), `bootstrap.rs` (sandbox and state locations),
`app.rs` (command routing), `repl/` (session state), `commands/` (run, agents,
export/import), `runtime/` (kernel, provider and tool assembly). Every other
module owns one command family or one frontend.

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
  → compact eligible Skill metadata
  → retrieve memory and checkpoint user input
  → context pressure check / layered compression
  → model streaming request
  → final + no continuation → optional StopGuard (default absent) → TurnFinished
  → pending input / approval / child / retry → wait, collect results, continue
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

Completion uses the shared `TurnState` / `TurnContinuation` state machine.
Default final text is streamed immediately and never routed through a reviewer.
See [agent-loop.md](agent-loop.md) for state, configuration and telemetry.

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
small Task Queue stores concrete model-created tasks through `task_queue` or typed
`ax_work_items` tool observations;
numbered/bulleted formatting is a hint and never creates a queue.
There is no separate planner call or fixed orchestration system prompt.
`task_queue start` accepts full input strings or `{title, input}` objects. Titles
are display metadata; isolated children receive the complete `input` and explicit
context rather than headings or sibling/controller history. The model may first
read data and then create the actual dynamically discovered instances.
An active same-goal queue that has not dispatched a task can be replaced by
another explicit `start`; its prior plan is archived as superseded. Dispatch is
checkpointed before tools or child provisioning so resumed in-flight work is not
silently replaced. Existing child identities, task outcomes or failures also
prevent replacement; a new goal remains the explicit way to supersede executed
work. Legacy string queues fall back to their full stored title as task input.


Queues belong to a `goal_id`, not a session. Ordinary user turns start a new goal,
supersede and archive any resumable old queue, and persist a fresh state head.
Supersede, archive and the replacement head reach durable storage in one
checkpoint, so an interrupted turn cannot restore a half-applied supersede.
`GoalTurn::Resume` restores only the explicitly requested saved goal ID. Budget
interruptions suspend that goal for explicit resume. CLI exposes `ax run --session
<ID> --resume-goal <GOAL_ID> <PROMPT>` and `--cancel-goal`; ACP uses prompt metadata
`_meta.axGoal` with `action` new/start/resume/cancel and `goal_id` where applicable.
Successful ACP prompt responses return the goal ID in `_meta.axGoal.goal_id`.

Tasks have pending/running/completed/failed/skipped states. Explicit `finish`
controls advance the queue; recovery before failure is an orchestration choice.
Tool failures and tool timeouts remain local, allowing independent tasks to
continue. Recovery records the failed step, call ID, tool and path resources as
advisory context. It never narrows scope or disables diagnostics, retries or
subsequent subtasks. Provider failures use classified attempt/time-bounded retry policy; configuration,
authentication, persistence and exhausted provider retries block the goal.

Queue states are active/summarizing/suspended/completed/blocked/cancelled/superseded.
Explicit global `block`/`cancel` controls and typed `ToolError::GlobalBlocked`
stop queue consumption immediately. A text-only response never advances a task:
with unfinished tasks it keeps the goal active; after terminal tasks it
completes the summary. The terminal response is checkpointed before output and
cached for reconnect without another model call or repeated response events.
Worker forks strip controller queue state and retain only parent goal identity,
so isolated worker contexts do not require manually created user sessions.

Queue checkpoints use the existing AgentState path; see [context.md](context.md),
[storage.md](storage.md) and [ADR 0008](adr/0008-durable-task-queue.md).

### Automatic isolated child execution

CLI, TUI and ACP/Crew prompt adapters all call the shared `run_prompt_with`
composition boundary. `child_runtime::configure_controller` uses one
`LocalChildHost::for_controller` constructor to bind the project scope, storage
exclusions, controller budget and independent child execution budget on every
prompt, including restored/preinitialized kernels. ACP's early command dispatch
still reaches this boundary through `run_session_prompt`; it adds no Agent Loop.
Children use the existing kernel loop and the same `ToolRegistry`, rebound to the
child's run context; they do not receive the controller queue or siblings'
messages. Plain single-task turns keep their existing loop. Embedders can opt in
via `with_child_host`.

Once the model explicitly sets `execution="children"`, the runtime stops
promoting one task per model round and dispatches the whole **ready frontier**
instead. `pending` becomes ready when every dependency is `Completed`; every
ready task whose declared `resources` do not overlap in-flight work is
provisioned and started in the same batch, bounded by the supervisor's
concurrency (`with_child_concurrency`, default 4). As children settle, newly
ready tasks are admitted immediately, so the controller never waits for one child
before starting the next. A failed or timed-out child records its receipt and the
frontier moves on. Cancelling or dropping the turn drops every child with it,
because children are futures owned by the turn rather than detached tasks.

Conflict is decided by the existing resource model: a task may declare
`resources` (path or name, with `write: true` when it modifies them) and two
tasks whose declared accesses overlap are never concurrent. Tool-level conflicts
are still serialized by the process-wide resource leases; nothing new was added.

Each child returns a structured `ChildResult` (status, summary, findings, changed
files, diff statistics, diagnostics, validation, artifacts, failure reason,
continuation hint, metrics). The controller model only receives the compact
`model_summary()` projection; the full record is durable and is read back with
the `child_result` control tool, so recovering detail never means re-running a
child. The controller enters summary/reporting after all children
are terminal. A receipt recovered from an earlier process short-circuits
provisioning, so a resumed session never re-runs a completed child. See
[ADR 0016](adr/0016-parallel-children-and-instructions.md).

### Project instructions, and asking the user

`instructions::InstructionResolver` resolves the instruction set for a turn
deterministically: the global file, `AGENTS.md` at the repository root,
`AGENTS.md` at every directory from the root down to the cwd
(`AGENTS.override.md` replaces the same level), then `.ax/rules/*.md` whose
`path`/glob scope matches a file the task names. Segments carry `source_path`,
scope, priority and provenance, the most specific level wins, and the result is
bounded by `ContextBudget::instructions_budget_tokens()` while keeping the
always-on chain. Nothing consults memory, embeddings or a semantic index.

Project instructions are their own context slot (`[ax-project-instructions]`),
separate from retrieved memory, skill instructions and system context; none of
the four stands in for another. They are re-resolved every turn, so they can
never go stale in a resumed session. See
[context.md](context.md#project-instructions).

`request_user_input` is a first-class structured control tool for questions that
materially change the result and cannot be answered from the repository. It is
never an authorization gate: permissions remain in the permission system. Asking
checkpoints the goal, queue, session and question, parks the queue in
`waiting_for_user` (never failed, never dropped) and suspends the run. The answer
is written back to the same tool call id and `GoalTurn::Answer` resumes the run
from that position without opening a new user turn.


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
workspace quota accounting and is preserved by GC. GC reclaims terminal and
expired workspaces; a child directory with no readable manifest is an orphan from
an interrupted provisioning step and is reclaimed after the same idle TTL, so it
cannot leak indefinitely.


### Goal-bound execution invariants

`ExecutionState` lives in the kernel independently of conversation compression.
It retains the original goal and identity, current step, expected output, task
scope, failed-step recovery binding, bounded real completion events and progress.
The execution model can declare step metadata through `_ax_execution` on an
ordinary tool call. Within a step, declared scopes may narrow; a new step may
select directories within the initial workspace after any observation. Scope
admission checks declared resources after typed result substitution, before
approval and execution. Empty results, no-match and failed calls all move the
step from `running` to `observed`, permitting retry, replan, next step or an
explicit failed task outcome without requiring positive progress.

Eight consecutive calls without a declared state transition trigger an advisory
`NoProgressDetector` projection. Read/list/search success is observation; new
successful mutations, explicit result evidence and repaired retries count as
progress. Repeated mutations/evidence are deduplicated. Neither this accounting
nor recovery, retry counts or exploration counts restrict tool admission.
Permissions, sandbox confinement, resource conflicts, dependencies, configured
budgets/timeouts and cancellation remain enforced. Unknown shell/MCP effects
retain exclusive resource leases. No separate planner/model request is made.
Legacy checkpoints restore their original declared scope before tool admission.
See [context.md](context.md), [tools.md](tools.md),
[ADR 0009](adr/0009-execution-invariants.md) and
[ADR 0014](adr/0014-advisory-execution-policy.md).


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

## Workspace runtime boundary

The independent `sandbox` crate sits below ToolRegistry and MCP stdio transport.
Agent, subagent and Skill execution reach SandboxManager before local OS effects.
Bound tools retain a manager and reuse a persistent Linux namespace broker; child
registries bind their own workspace. Permission remains independent. See
[security.md](security.md) and [ADR 0013](adr/0013-workspace-runtime-sandbox.md)
for backend requirements, lifecycle capabilities and platform limitations.

Current coding execution policy, task-level Git workspaces, optional stop verification
and artifact staging/export are described in [coding-harness.md](coding-harness.md).

### Native provider dispatch

`model::provider_adapter` constructs Chat Completions, Azure Responses or native
Anthropic/Gemini/Vertex/Bedrock/Radius adapters. CLI inference and catalog refresh
share the same construction path. Native modules own protocol translation and
authentication; AWS/Google credential discovery remains lazy. Optional assistant
`provider_metadata` preserves signed native blocks through session checkpoints
and context budgeting without adding model calls. See [providers](providers.md)
and [ADR 0020](adr/0020-native-model-providers.md).

## Optional Distributed Collaboration

The CLI composition root conditionally registers the `collaboration` Tool only when worker environment bindings are present. `distributed_client`, `distributed_tool` and `distributed_worker` provide scoped REST, Tool adaptation and per-lease ACP execution. Core reasoning, Subagents, Tasks, Sessions, Memory, Skills and MCP are unchanged. `ax crew worker` is separate from the existing device bridge. See [distributed-collaboration.md](distributed-collaboration.md) and [ADR 0021](adr/0021-durable-distributed-collaboration.md).
