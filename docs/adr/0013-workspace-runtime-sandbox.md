# 0013: Workspace runtime sandbox

Status: accepted

## Context

Tool-specific path checks and permission prompts cannot constrain shell indirection,
subprocesses or local MCP servers. AX needs workspace autonomy with an OS boundary
that survives model, Skill and subagent choices, without requiring Docker.

## Decision

Introduce an independent sandbox crate and central manager below tools. Linux uses
persistent bubblewrap namespaces, pinned filesystem objects, seccomp and rlimits.
Builtin filesystem tools run in private workers; shell and stdio MCP spawn through
the same executor. Undeclared execution boundaries deny by default. Confined modes
fail closed, including on currently unsupported platforms. Permission remains a
separate layer and cannot silently weaken a running sandbox.

Crew selects locally registered workspace identities. Child runtimes use private
Git clones and narrow provisioning capabilities. A retained manager reuses the
namespace across calls and kills descendants at teardown.

## Consequences

The agent loop keeps its existing Permission and tool orchestration. Linux deployment
requires the documented kernel and userspace capabilities. Windows/macOS require
native backends before using confined modes. Unverified filesystems and hardlinked
workspace inputs are rejected. Trusted runtime services remain responsible for AX
state, while arbitrary tool effects run only through the sandbox.

See [security.md](../security.md) for enforcement, limitations and executable tests.
