# 0018. Continuation-driven completion

Status: accepted

## Context

The coding harness reviewed every text-only final with a second model request and
withheld streamed text. That increased ordinary-answer latency and confused real
tool/task continuation with optional verification.

## Decision

All entry points share `TurnState` and `TurnContinuation`. Final plus no pending
runtime work completes directly. Wait states await execution/input rather than
polling a reviewer. No task classifier, prose heuristic or historical tool-use
flag participates in completion.

Streaming is immediate. Stop verification is an optional `StopGuard`, absent by
default. Deterministic checks precede explicitly selected model verification;
custom command/test/policy/agent guards use the same extension boundary. Worker
guards must be configured for their own goal rather than inheriting controller
deliverable paths. Execution and guard model requests are reported separately.

## Consequences

Ordinary turns have a theoretical minimum of one model request, zero reviewer
requests. Model-owned planning remains model-owned; the kernel cannot prove that
every implied user deliverable was identified. Users needing stronger completion
conditions opt into concrete verification rules. Legacy audit records remain
recoverable without recreating a mandatory review stage.

See [agent-loop.md](../agent-loop.md) for the runtime and configuration contract.
