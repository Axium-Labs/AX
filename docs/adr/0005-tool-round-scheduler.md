# 0005. Dependency-aware tool-round scheduling

Status: accepted

## Context

Sequential execution forces independent model tool calls to wait for each
other. Concurrency based on tool names, approval levels or prompt wording
cannot safely detect shared filesystem, Git, database and Memory effects.
Concurrent same-name calls also require real call IDs in lifecycle events.

## Decision

Tools declare read/write resource accesses through `Tool::resources`. Unknown
effects use a global exclusive lease; permissions remain a separate contract.
The Runtime builds a DAG from typed result references, explicit dependency
metadata and resource conflicts. It runs at most four ready futures by default,
with a bounded configurable limit, and retains deterministic ordering for
conflicting effects unless a data dependency requires the reverse order.

Path resources are canonicalized and hierarchical. Memory locks the actual
database file. Shell/Git and unclassified external tools remain conservative.
Resource leases are shared process-wide across kernels and drop on cancellation.
Interactive approvals are serialized independently of execution.

Results retain their original tool call IDs. Raw checkpoints follow completion
order; the next model request receives the completed round in call order.
Typed result references and dependency metadata are part of the runtime tool
schema, and their schema cost is included in `ContextBudget`.

## Consequences

Independent asynchronous work can overlap without unrestricted spawning.
Failed producers prevent dependent side effects, while unrelated calls can
continue after an error. Cycles and invalid IDs fail before execution, and
existing interruption recovery still produces a well-formed history.

Tool authors must accurately declare effects to gain concurrency; undeclared
tools sacrifice parallelism for safety. Locks coordinate AX invocations within
one process, not outside processes or separate AX executables. Synchronous CPU
work is not made parallel by the in-task asynchronous scheduler.
