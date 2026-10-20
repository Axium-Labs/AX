# 0033. Sandbox process, protocol and cache boundaries

Status: accepted

## Context

Error strings confused ordinary command failures with confinement failures.
Shared tool/MCP PID namespaces and serialized command argv exposed server secrets.
Killing an immediate child left background jobs alive, and synchronous safety
queries performed namespace preparation.

## Decision

Tag worker outcomes structurally. Object authorization and confinement or worker
startup failures become SandboxViolation; ordinary tool failures remain Execution.
Keep tool brokers workspace scoped; allocate a broker/PID namespace per stdio MCP
transport. Send bounded private socket frames, independently of child stdin.
Kill command process groups on cancellation and exit; preserve Linux signal codes.
Prepare session tool namespaces on blocking workers, cache successes and rebuild
dead brokers once without replaying commands or falling back to the host.

Strict enforces denied networking; workspace retains automatic approval and
networking. Use RLIMIT_DATA and a CPU-scaled RLIMIT_NPROC (256 per detected CPU),
without requiring privileged cgroups. Mount a workspace-keyed cache from AX_HOME
outside the workspace, runtime assets, and generated current uid/gid identity.

## Consequences

Workspace mode cannot prevent workspace-data exfiltration. Generated trees follow
discovery skips for hardlink screening and remain trusted inputs. Process groups
cover ordinary jobs, not deliberate setsid escapes; namespace destruction remains
the final lifecycle boundary. RLIMIT_DATA is neither an RSS nor aggregate quota.
MCP namespaces share workspace/cache files but not processes. Off retains host
execution semantics. See [security.md](../security.md) for the exact boundary.
