# Continuation-driven completion

The shared kernel runs `model → execute tools/tasks/children → append results →
model`. A final response with no continuation ends the turn immediately. It does
not inspect prompt keywords, task complexity, whether files changed, or whether
tools, tests, shell commands or subagents were used earlier. Ordinary answers and
code explanations require one execution model request and zero reviewer requests.

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

## Optional Stop Guards

No guard is installed by default. The coding harness only installs execution
policy/environment and typed task admission; enabling it does not enable review.

Global `$AX_HOME/config.json` accepts:

```json
{
  "verification": {
    "mode": "off",
    "deliverables": [],
    "required_successful_calls": []
  }
}
```

Modes shipped here are `off`, `deterministic`, and `model`. A project can override
the whole verification object in `.ax/verification.json`, for example:

```json
{
  "mode": "deterministic",
  "deliverables": ["dist/report.json"],
  "required_successful_calls": []
}
```

Relative paths resolve against the project root. Deterministic mode checks
runtime readiness, deliverable existence and explicitly configured successful
tool **call IDs**. It never infers test success from shell text, file names or
the assistant's assertion. Success evidence comes from structured tool envelopes,
including the current raw checkpoint history when context was compressed. File
existence alone does not establish freshness or semantic correctness.

Model mode runs those deterministic checks first. Only after they pass does it
make an additional streaming provider request with one `stop_decision` schema.
The verifier cannot execute tools; denial supplies a concrete reason back to the
ordinary execution loop. Configured model step budgets include guard requests;
provider retry policy and turn timeout still apply. Guard text is not sent as a
second final response. Normal assistant text streams immediately, while the
explicit guard blocks only `TurnFinished`.

Embedders can install `Arc<dyn StopGuard>` with `with_stop_guard`. Its async
`evaluate(&TurnState)` returns `Allow` or `Continue { reason }`. `name()` labels
observability and `uses_model()` declares a model-backed guard for budget admission.
Command, policy, test and agent guards can implement this extension; `agent` is
not a built-in configuration mode in this change. A guard that calls models should
record requests in `state.guard_model_requests`. Controller guard rules are not
implicitly inherited by worker forks: a controller's report paths and call IDs
belong to its goal. A host may explicitly configure a child guard for child-local
requirements.

## Observability and recovery

Each model step emits `Continuation { goal_id, step, continuation }`. Completion
emits `Completion { goal_id, completion, model_steps, guard_model_requests, tools,
guard }`, followed by `TurnFinished`. Examples of the corresponding values:

```text
step=1 continuation=tool_call
step=2 continuation=complete
completion=direct model_steps=2 guard_model_requests=0 tools=1

completion=stop_guard model_steps=1 guard_model_requests=1 guard=model
```

`model_steps` counts execution-loop inference steps, not child dispatch batches or
guard requests. `StopGuardEvaluated` records the guard name, decision and number of
actual guard provider requests, including retries. Goal IDs distinguish controller
and child events. `AX_EVENT_LOG` writes JSONL from the shared prompt boundary for
CLI, TUI and ACP, without adding UI noise or model requests when unset.

Default finals are checkpointed directly. Explicitly guarded child finals have a
pending marker persisted in the same checkpoint as the assistant message; only a
durable allow marker permits recovering that final as terminal. Provider-declared
nonfinal text cannot recover as a final receipt. Old `completion_check` and
`[ax-completion-pending]` records are read only for legacy recovery compatibility;
no default path generates them. Raw history remains intact.

## Verification and references

`test/harness/continuation.rs` and `subagent_continuation.rs` assert request counts,
stream timing, real pending approval/retry/input, queued tasks, unfinished and
unconsumed subagents, deterministic checks, model guard opt-in and denial/recovery.
Existing frontend/workspace fixtures reject any default completion-review prompt.

Design references: [OpenAI's Codex agent loop](https://openai.com/index/unrolling-the-codex-agent-loop/)
and [Claude Code Stop hooks](https://code.claude.com/docs/en/hooks#stop). AX adopts
tool/result-driven continuation and optional stop hooks, without copying a fixed
semantic verifier into the core loop.
