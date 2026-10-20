# ADR 0030: Unified model-selected delegation

Status: Accepted, 2026-10-20.

## Context

Ordinary subagents and queue children used different concurrency allowances.
Queue children did not inherit the ordinary absolute depth budget, so depth 0
was not a universal local delegation switch. Typed reader inventories selected
children automatically even though data mapping did not express a delegation
decision. Guidance also needed to cover work beyond programming roles.

## Decision

Keep one execution loop and let the main model judge task decomposition. General
policy encourages bounded delegation when focused context, independent checking
or parallel progress justifies coordination, including research, writing,
document review, analysis, planning and coding. It does not require splitting,
fixed role names, a complexity classifier or an extra planner request. User
ordering, scope, authorization and requests to work alone remain authoritative.

Both local delegation paths share the existing concurrency pool and absolute
depth/configuration (defaults 8 / 1). Slots cover provisioning through settlement;
waiting ancestors occupy slots. Nested exhausted admission rejects locally to
avoid waiting on an occupied ancestor. Ordinary subagents retain their finite
64-admission safeguard; queue batch size remains independent of concurrency.
The compatible `with_child_concurrency` setter writes the same limit.

The shared child binding inherits permission ceilings, allowed tools, effective
configuration, depth plus one and the pool, then rebinds the host to the child
workspace. Runtime checks enforce disabled/depth/tool boundaries both at queue
admission and dispatch, including restored work. Controller queue control cannot
be invoked by a child. Disabled restored queues keep task identities, child
handles and receipts, return to controller execution and give the model local
feedback. No permission grant is inferred from prompt guidance or availability.

New typed inventories default to controller execution. Explicit
`task_queue action=configure` changes execution mode without replacing task
identity, progress, dependencies or receipts. Appended records preserve an
existing explicit children choice only while current runtime limits allow it.
The bounded per-turn availability context exposes effective limits without
performing a model/network request or creating a child.

## Consequences

Queue concurrency defaults to 8 instead of its former independent 4. Depth 0
disables every new local child, including a resumed queue. Queue children may use
ordinary nested delegation when depth and inherited tools permit it. Explicit
model selection after a reader can require a configuration call; it prevents a
data observation from silently choosing an execution strategy. Model judgment
can still choose an unhelpful split; runtime limits bound resource/permission
effects but do not certify that a split improves quality.
