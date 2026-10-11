# Continuation-driven completion

The shared kernel runs `model → execute tools/tasks/children → append results →
model`. A final response with no continuation ends the turn immediately. It does
not inspect prompt keywords, task complexity, whether files changed, or whether
tools, tests, shell commands or subagents were used earlier. Ordinary answers and
code explanations require one execution model request and zero reviewer requests.

Every turn executes under the coding harness: the user request defines the
work, and the runtime drives it to a terminal state. The runtime prompt and
the per-capability guidance are injected at the start of the turn. See
[agent-runtime.md](agent-runtime.md).

## Runtime state

`continuation::TurnState` projects pending tool calls/results, task queue entries,
running children, unconsumed child results, pending approvals, user input/steer,
retry activity, required actions and provider continuation signals into:

```rust
enum TurnContinuation {
    Continue(ContinuationReason),
    Wait(WaitReason),
    Complete,
}
```

`needs_follow_up` is true for Continue or Wait. The scheduler awaits tool commands
and approval futures before another model step; retry futures also remain inside
the provider step. Cancellation-safe counters track approvals and retries.
Delegated queue children are awaited by the existing bounded supervisor. Optional
subagents are collected before final completion if their receipts were not yet
returned. New results force a model step; successful consumption clears them.
`request_user_input` suspends with `WaitingForUser` and resumes at the original
call ID. Hosts can enqueue live input through `kernel.turn_input().steer(...)`;
input arriving during streaming or a blocking guard is consumed before completion.
This API does not start a turn by itself.

Provider finish reasons `length`, `max_tokens`, `incomplete`, `pause_turn`,
`tool_calls` and `function_call` request continuation. Missing finish reasons
remain compatible with providers that signal final through an empty tool list.
There is no prose matching. Explicit cancellation, global failures and execution
budgets keep their distinct stop/suspension semantics.

CLI, REPL, TUI, `ax run`, ACP and the AX Crew ACP bridge use this same kernel.
Frontends render events; they do not run a completion reviewer or decide that an
assistant message is terminal. `ContentDelta` is forwarded as tokens arrive,
including when a queue is active. `TurnFinished` is AX's turn-complete event.

## Loop hygiene

Consecutive identical tool calls (same tool, same canonical arguments) are
detected per turn. At 3, 5 and 8 consecutive identical calls the runtime injects
one advisory `[ax-loop-hygiene]` system reminder asking the model to inspect the
previous result, change approach, or conclude the task is satisfied. The call is
still executed; the reminder only enriches the next request. There is no low
global tool-call cap — long, productive tasks still make many calls. The chain
resets when the call changes and at the start of each turn. See
`crates/core/src/loop_hygiene.rs`.

## Optional Goal

Ordinary tasks finish directly without extra validation or provider requests.
Explicit `/goal <target>` adds target-scoped deterministic or independent model
verification to this same loop. A denial supplies concrete continuation reasons;
blocked, failed, paused and cancelled states remain distinct from completion.
Goal criteria, limits and consumption live in the existing queue checkpoint with
the same goal ID. Configuration templates do not activate ordinary tasks.

See [goal.md](goal.md) for commands, lifecycle, resource admission, migration and
recovery. `GoalEvaluator` is the single extension boundary evolved from the old
stop verifier. Workers do not inherit controller criteria.

## Observability and recovery

Each execution model step emits Continuation. Optional Goal emits GoalUpdated
and GoalEvaluated (including the structured decision). Completion reports
`model_steps`, `goal_model_requests`, tools and evaluator, followed by TurnFinished.
Default completion is `direct`; verified completion is `goal`. AX_EVENT_LOG records
these events through the shared CLI/TUI/ACP prompt boundary. Verifier output never
becomes a second final answer.

Pending Goal and provider continuation markers prevent recovering premature
child finals. Only a durable Goal allow marker permits recovering a verified final.
Old completion_check and pending/allowed records are compatibility data only.
Raw history remains intact.

## Verification and references

`test/harness/continuation.rs` and `subagent_continuation.rs` assert request counts,
stream timing, real pending approval/retry/input, queued tasks, unfinished and
unconsumed subagents, deterministic checks, model Goal opt-in and denial/recovery.
`test/harness/runtime_neutrality.rs` asserts the runtime contract: a plain request
is answered directly and its goal queue completes, context cannot hijack it,
explicit investigation and fixes run to completion, a long-running request
creates a resumable queue, and repeated identical calls are reminded at 3/5/8
rather than blocked.
Existing frontend/workspace fixtures reject any default completion-review prompt.

Design references: [OpenAI's Codex agent loop](https://openai.com/index/unrolling-the-codex-agent-loop/)
and [Claude Code Stop hooks](https://code.claude.com/docs/en/hooks#stop). AX adopts
tool/result-driven continuation and optional target-driven Goal, without copying a fixed
semantic verifier into the core loop.
