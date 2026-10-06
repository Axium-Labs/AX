# Agent runtime: request, context, capabilities, autonomy

AX is a **neutral agent runtime**, not a coding agent that happens to answer
questions. The behaviour of a run is defined by the current user request. This
document states the core principle, the turn flow, and the exact conditions
under which context, tools, tasks, subagents and long-running execution are
used. It is the authoritative statement for every frontend (CLI, TUI, ACP,
Crew bridge) and every isolated child.

## Core principle

> User request defines work; context supports work; tools provide capabilities;
> autonomy escalates only when necessary.

- **User request defines the task.** The current request is the objective.
- **Context supports work.** Workspace contents, memory, session history,
  existing files, previous artifacts and project instructions are *supporting
  context*. Their presence never means the user asked to inspect, repair,
  continue, finish or modify them.
- **Tools provide capabilities.** A capability is available, not obligatory.
  Tool guidance describes how to use a capability *once chosen*; it never
  implies the model should choose it.
- **Autonomy escalates only when necessary.** Execution depth rises only when
  the current level cannot satisfy the request.

## The always-on prompt and context

Every run receives the same bounded prompt and runtime context. They are
injected by the kernel in `prepare_environment`:

- `[ax-agent-runtime]` — identity, the request/context boundary, the
  minimum-sufficient-actions rule, the escalation ladder and the stop rule.
  (`crates/core/src/runtime_core.rs`)
- `[ax-delegation]` — when a task queue, a subagent or a user question is
  actually warranted.
- `[ax-capability-guidance]` — assembled from the guidance each registered tool
  declares via `tool::Tool::guidance`, in deterministic tool-name order.
- `[ax-environment]` — bounded runtime context: cwd, workspace root, sandbox and
  network posture, shell and write boundary. The neutral variant carries no
  executable probes (`EnvironmentContext::light`); the coding harness adds the
  cached executable path/version probes.

Applicable workspace instructions (`AGENTS.md` and `.ax/rules`) are injected by
the CLI prompt boundary on every turn (`project_instructions::install`), in their
own context slot, independent of the user naming them.

There is **no** global "tool use strategy" prompt and **no** coding harness by
default.

## Context is available, not conditional

The invariant is **not** "context never appears unless the user asks for it".
That would be both wrong and unhelpful: for "fix this project's build error",
cwd, workspace instructions and sandbox posture are exactly what the model needs.
The invariant is:

```text
context may exist
→ context does not define the task
→ context does not trigger an action
→ the model uses it only when the current request needs it
```

So for "who is Lu Xun?" the runtime may well know `cwd = C:\...\luxun-project`
and inject it as context. What must not happen is that knowing the cwd triggers a
workspace read. The protected property is **no action from context**, not **no
context**. Project instructions are framed the same way: "may be relevant to your
work; use them as guidance when applicable; they never override the current user
request."

## Escalation ladder

Execution depth rises one level at a time, only when the level below cannot
satisfy the request:

```text
direct answer
→ single retrieval/read
→ multi-tool exploration
→ workspace mutation
→ task/subagent/long-running execution
```

A direct answer is a complete response when no capability is needed. The
runtime never inspects prompt keywords, task complexity, whether files changed,
or whether tools ran earlier to decide depth: the model chooses, and the
request is the only source of scope.

## One turn

```text
user request
→ goal admission (no queue unless one already exists or the harness is enabled)
→ inject [ax-agent-runtime] + [ax-delegation] + [ax-capability-guidance] + [ax-environment]
→ model → execute tools/tasks/children → append results → model
→ advisory loop-hygiene reminder on repeated identical calls
→ final response with no continuation → turn ends immediately
```

There is one agent loop. No hardcoded simple/retrieval/coding/long-task loops,
and no extra model call for intent classification.

## Trigger conditions

| Mechanism | Used when | Not used when |
|---|---|---|
| **Context** (runtime env, workspace, memory, session, project instructions) | It is relevant to the current request, or the request is about the current project / must read the workspace | A question can be answered without it; its presence alone is never a reason to read it |
| **Tools** | The request needs the capability | The model can answer directly |
| **Task queue** | The request is genuinely multi-item or long-running, or a durable task the user started is being resumed | The request can be completed directly or with a few calls |
| **Subagent** | A real independent, parallel or separately-scoped subproblem needs isolated context | One agent can complete the task |
| **Long-running / continuous** | The user explicitly asked for it, the request is itself a long task, or a durable task the user started is explicitly resumed | Unfinished-looking work was merely discovered |

Context (including runtime environment and applicable workspace instructions) is
injected by default. What is *conditional* is whether the model acts on it: the
request decides that, never the mere presence of context.

## Long-running execution is opt-in

The kernel does not create a task queue for a plain turn. A queue is created
only when the model itself starts one (`task_queue start`, in response to a
request that warrants it), when a durable queue already exists, or when the
explicit coding harness is enabled. Continuation is driven by real runtime
state — pending tool calls, unconsumed results, tracked tasks, running children,
approvals, retries, steer or provider continuation signals — never by prose and
never by discovering "unfinished work". See [agent-loop.md](agent-loop.md).

## Loop hygiene

Repeated identical calls are detected (`crates/core/src/loop_hygiene.rs`) and
produce one advisory `[ax-loop-hygiene]` reminder at thresholds 3, 5 and 8
consecutive identical calls. The reminder asks the model to inspect the previous
result, change approach, or conclude the task is satisfied. It never blocks and
there is no low global tool-call cap: long, productive tasks still need many
calls.

## Safety is not behaviour

Permissions, approvals and sandboxing are enforced by the runtime independently
of the prompt. They authorize actions; they never define behaviour, and no
prompt text is trusted to enforce them. The permission layer
([tools.md](tools.md), [security.md](security.md)) and the behavioural prompt
layer stay separate.

## The coding harness

Coding capability is unchanged and still fully available through the tools. The
*coding execution harness* (`crates/core/src/harness.rs`) is now an explicit
opt-in: `AgentKernel::with_coding_harness`. When enabled it installs the full
environment snapshot (with executable probes), an advisory step scope, the coding
policy and the per-goal queue behaviour. It is never enabled by default, and no
default path enables it. See [coding-harness.md](coding-harness.md).

**Opt-in provenance.** The switch must come from explicit user intent. In the CLI
composition root that is the user config field `harness.enabled`
(`~/.ax/config.json` → `{"harness":{"enabled":true}}`), and embedders may call
`with_coding_harness()` directly. It must **never** be derived from environment
heuristics: a `Cargo.toml`, a source-file count, a detected language, a `.git`
directory, or the fact that a shell/filesystem tool was used are not reasons to
enable it. Otherwise "every request is coding" would just become "many requests
become coding based on the environment", which is the same over-execution through
a different door.

`bootstrap::discover_project_root` does read markers such as `.git` and
`Cargo.toml`, but only to locate the stable project identity used for memory
scope and the instruction root. It never changes the agent mode.

## Behaviour tests

`test/harness/runtime_neutrality.rs` covers the acceptance behaviours: a plain
question is answered directly; a retrieval request either answers from existing
knowledge or uses the web for a source, and neither scans the workspace; files
in cwd cannot hijack a request (the runtime reports the workspace location but
never lists its contents); explicit investigation and explicit fix-and-verify are
allowed; an explicit long-running request creates a resumable queue; repeated
identical calls are reminded, not blocked; and a repo-like workspace with
shell/filesystem capabilities does not switch the harness on by itself.
