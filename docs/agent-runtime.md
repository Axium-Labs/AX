# Agent runtime: request, execution, capabilities

AX is a **coding execution harness**, in the `DeepSeek Harness` lineage. The
current user request defines the work, and the runtime drives requested
deliverables to a terminal state: completed, failed with evidence, or waiting
on a user-exclusive decision. This document states the core contract, the turn
flow, and how context, tools, tasks, subagents and repetition are handled. It
is the authoritative statement for every frontend (CLI, TUI, ACP, Crew bridge)
and every isolated child.

## Core contract

> The user request defines the work; context supports the work; tools provide
> capabilities; execution continues until the requested result is terminal.

- **The request defines the work.** Workspace contents, session history,
  existing files and project instructions are *supporting context*: they may be
  relevant, but their presence never widens the request.
- **Execution is tracked.** Every goal runs on an active task queue
  (`begin_goal`). Queue items are concrete user deliverables — never a
  discovery checklist — and the turn ends only when tracked work is terminal.
- **Tools provide capabilities.** A capability's guidance describes how to use
  it *once chosen*; it never implies the model should choose it.
- **No mode switch.** There is no configuration flag and no builder method that
  turns the harness off. Composition — which tools and providers a host
  registers — is the only variation point.

## The always-on prompt and context

Every run receives the same bounded prompt and runtime context, injected by the
kernel in `prepare_environment`:

- `[ax-agent-runtime]` — identity, the request/context boundary, the
  minimum-sufficient-actions rule and the permission boundary.
  (`crates/core/src/runtime_core.rs`)
- `[ax-capability-guidance]` — assembled from the guidance each registered tool
  declares via `tool::Tool::guidance`, in deterministic tool-name order.
- `[ax-environment]` — the full environment snapshot: cwd, workspace root,
  sandbox and network posture, shell contract and cached executable probes
  (`EnvironmentContext::detect`).
- `[ax-coding-harness]` — the execution policy
  (`crates/core/src/harness.rs::POLICY`): recoverable setup observations,
  queue discipline, child-input contracts and the terminal-state requirement.

Applicable workspace instructions (`AGENTS.md` and `.ax/rules`) are injected by
the CLI prompt boundary on every turn (`project_instructions::install`), in
their own context slot. There is no global "tool use strategy" prompt.

## One turn

```text
user request
→ goal admission (every goal gets an active task queue)
→ inject [ax-agent-runtime] + [ax-capability-guidance] + [ax-environment] + [ax-coding-harness]
→ model → execute tools/tasks/children → append results → model
→ advisory loop-hygiene reminder on repeated identical calls
→ queue terminal and no continuation → turn ends
```

There is one agent loop. No hardcoded simple/retrieval/coding/long-task loops,
and no extra model call for intent classification.

## Trigger conditions

| Mechanism | Used when | Not used when |
|---|---|---|
| **Context** (runtime env, workspace, memory, session, project instructions) | It is relevant to the current request | Its presence alone is never a reason to act on it |
| **Tools** | The request needs the capability | The model can satisfy the request directly |
| **Task queue** | The request is multi-item or long-running | The request completes directly — an empty goal queue simply completes with the answer |
| **Subagent** | A real independent, parallel or separately-scoped subproblem needs isolated context | One agent can complete the task |
| **request_user_input** | A user-exclusive decision suspends the goal | Permission questions — dangerous operations are authorized by the permission system |

Context is injected by default; what is *conditional* is whether the model acts
on it: the request decides that, never the mere presence of context.

## Loop hygiene

Repeated identical calls are detected (`crates/core/src/loop_hygiene.rs`) and
produce one advisory `[ax-loop-hygiene]` reminder at thresholds 3, 5 and 8
consecutive identical calls. The reminder asks the model to inspect the previous
result, change approach, or conclude the task is satisfied. It never blocks and
there is no global tool-call cap: long, productive tasks still need many calls.

## Global stops are evidenced

The model cannot end a goal by declaring a global blocker. A
`task_queue block` action is accepted only when it cites call ids whose results
carry a real `ToolResult.global_blocker` (a `ToolError::GlobalBlocked`
classification by the runtime). Prose, untested resource assumptions or local
failures are rejected, and the model must recover or continue.

## Safety is not behaviour

Permissions, approvals and sandboxing are enforced by the runtime independently
of the prompt. They authorize actions; they never define behaviour, and no
prompt text is trusted to enforce them. The permission layer
([tools.md](tools.md), [security.md](security.md)) and the behavioural prompt
layer stay separate. Model-declared step subscopes are advisory: tools always
enforce the workspace boundary, so a mistaken subscope cannot lock recovery.

## Behaviour tests

`test/harness/runtime_neutrality.rs` covers the acceptance behaviours: every
run carries the harness context; a plain question is answered directly and its
goal queue completes; a retrieval request either answers from existing
knowledge or uses the web for a source, and neither scans the workspace; files
in cwd cannot hijack a request (the runtime reports the workspace location but
never lists its contents); explicit investigation and explicit fix-and-verify
run to completion; a long-running request creates a resumable queue; repeated
identical calls are reminded at 3/5/8, not blocked; and a real global blocker
ends a goal without a summary model call.
