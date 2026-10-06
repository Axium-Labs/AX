# Distributed collaboration (optional)

This is an additive CLI adapter and Tool, introduced in AX 0.3.7 with AXCrew 0.3.3.
Ordinary AX, local Subagents, Task Queue and the existing Crew device bridge
continue to work independently. No cluster connection is made during ordinary
AX startup.

## Responsibilities and identity

`Host → AX Instance → Execution` is a hierarchy, not a one-to-one mapping.
A Host is a real machine with shared resource capacity. An AX Instance is an
independent runtime configuration and credential on that Host. Each instance
can run several Executions, each of which uses an existing ACP session and may
create local Subagents. A distributed Task is durable work owned by AXCrew;
it is distinct from an AX-local Task, Session or Subagent.

AX keeps model reasoning, planning, tools, skills, MCP, memory and local
execution. AXCrew owns assignment, leases, placement, retry, cancellation and
workflow state. Agents request assistance; they do not assign ownership.

The collaboration cycle is asynchronous:

```text
Task decomposition → capability scheduling → cross-machine execution
→ Event / Artifact return → state persistence
→ failure recovery / replanning → continued execution
```

AX instances do not communicate directly. Workers pull assignments and renew
leases every five seconds. Tasks, observations, immutable artifact references
and versioned Workflow State survive the disappearance of the AX that originally
decomposed the work. A reassigned root task receives its checkpoint and known
workflow tasks/results and can continue planning without a permanent Coordinator
Agent. This resumes durable work, not the previous model's complete session.

## Setup

1. Run a current AXCrew server and open the desktop **Distributed** sidebar page.
2. Add an AX instance with a stable Host ID, name, project mapping, concurrency
   and delegation policy. No CPU/GPU/RAM or Skill/MCP/Tool/model fields are needed
   at enrollment. Instances on the same physical Host must reuse its Host ID.
3. Download `worker.json`. Map the same logical project ID to each machine's
   local source directory, such as `D:\AX`, `/srv/AX` or `/workspace/AX`.
4. Check local model credentials, enabled capabilities, paths and permissions,
   then run `ax crew worker worker.json` on that Host.
5. After connection, inspect automatically detected hardware under **Hosts**.
   Use **AX instances → Configure capabilities** to add roles, Skill/MCP/Tool
   names, a model and other settings. Merge the downloaded settings into the
   existing local worker config, install/enable local dependencies and restart.
   The page shows pending configuration until AX reports matching capabilities.
6. Create a task through the management page or distributed API. AXCrew selects
   an eligible AX and reserves resources across all AX instances on its Host.

Example configuration (replace the credential and IDs from enrollment):

```json
{
  "gateway": "https://crew.example.com",
  "token": "instance-specific-secret",
  "instance_id": "enrolled-instance-id",
  "projects": {"project-ax": "/srv/AX"},
  "execution_root": "./executions/ax-test",
  "ax_home": "./ax-home/ax-test",
  "max_executions": 2,
  "provider": "deepseek",
  "model": "deepseek-chat",
  "skills": [],
  "mcp": [],
  "permission_profile": "ask",
  "sandbox": "strict"
}
```

Paths are relative to the configuration file unless absolute. `execution_root`
must be outside project sources. `ax_home` is optional; use separate homes for
independent instance memory/configuration. Optional `skills_dir` and `mcp_config`
select local configuration files. Advertised models must match the explicit
model setting; advertised Skill/MCP names must be enabled in worker settings.
Concurrency and project mappings must match enrollment. Optional `roles`, `tools`
and `environments` select capability metadata. Tools must name AX built-ins; MCP
servers use the separate MCP selection. Default tools advertise built-ins and the
default environment is the OS name. Explicit Skill/MCP selections and a configured
model are reported by the worker, without copying model credentials or memory.
These selections are declarations of enabled configuration, not a health check
of every Skill/MCP service. Pending admin settings are checked during worker startup.

Remote connections require HTTPS with a valid certificate; plain HTTP is allowed
only for localhost/loopback. The worker initiates outbound connections and needs
no inbound AX port. Keep worker config private: it contains a scoped credential.
Use AXCrew's existing admin token and a TLS reverse proxy for remote deployment.

`ask` is the default. An unattended worker denies requests requiring interaction.
For autonomous mutations, explicitly preauthorize the local `allow` profile;
existing workspace sandbox and policy rules still apply. Cluster capabilities
never override local permissions. A stalled user interaction is reported as
failed work rather than a successful distributed result.

## Automatic Host inventory

Only `ax crew worker` starts inventory detection. A background native probe has a
10-second deadline and refreshes every 60 seconds, independently of the five-second
lease heartbeat. Failed heartbeat delivery retries the detected inventory.
Windows uses CIM for logical CPU count, physical RAM and PCI video controllers;
Linux reads `/proc` and PCI display devices in `/sys`; macOS uses `sysctl` and
`system_profiler`. The worker reports hostname, OS/architecture, CPU name/count,
RAM MiB, GPU count/names and probe errors. AXCrew timestamps accepted reports and
persists one shared capacity per Host, regardless of the number of AX instances.

Unknown RAM/GPU are reported as null, displayed as unknown, and reserve zero
available capacity for tasks requesting those resources. Unknown GPU does not
mean that no GPUs exist. GPU inventory includes integrated and discrete adapters;
it is not a count of CUDA devices. CPU count is logical, not physical core count.
Inventory and online status are separate: offline Hosts retain their last detected
configuration and timestamp. These figures are hardware capacity, not utilization,
OS quotas or GPU isolation. Normal single-machine AX startup is unaffected.

## Collaboration Tool

Workers inject an opt-in `collaboration` Tool into the ordinary
runtime. It uses these actions:

| Action | Purpose |
|---|---|
| `catalog` | Inspect AX roles/capabilities and Host resources |
| `delegate` | Submit a child with required capabilities/resources, summaries, dependency IDs and artifact IDs |
| `status`, `wait` | Read durable task/workflow state, results, errors and observations; wait is bounded |
| `observe` | Persist an observation for the current execution |
| `cancel`, `retry` | Request central cancellation or retry, subject to ownership/policy |
| `publish_artifact` | Publish a file inside the execution workspace as an immutable object |
| `read_artifact` | Verify and read a bounded text view of an artifact |
| `checkpoint_workflow` | Compare-and-swap a bounded JSON checkpoint using the current workflow revision |

Supply a stable `request_id` for each logical step. Replayed delegation after a
root's reassignment reuses the same child, even when the new owner is another
AX. Use a new request ID for a genuinely new test/replanning attempt. Without
an explicit ID, the Tool derives one from the submission content.

Checkpoint meaningful stages and artifact/task references. On a revision
conflict, inspect current Workflow State before updating it. An unsuccessful
dependency blocks dependent tasks; to analyze a failed test, create a new task
with its failure summary/Artifact references instead of requiring that failed
task as a successful dependency. Successful test execution can return a failing
test report: task execution status and test outcome are separate concepts.

## Workspace and artifacts

Each `(task_id, generation)` gets an isolated directory. Git projects use a
separate clone with independent Git metadata and no hardlinks, detached at the
requested commit (or source HEAD). Dirty source changes are not copied. Share
uncommitted work as a Patch artifact. Non-Git projects get a file snapshot;
`.ax`, `.git`, `target` and `node_modules` are excluded, symlinks/special files
are rejected, and commit pinning is unavailable.

Input artifacts are hash-verified and written under `.distributed-inputs/` by
artifact ID. Patch artifacts are checked and applied to Git workspaces. Completed
Git work can emit a changed-workspace patch and an execution result report artifact;
non-Git generated outputs should be explicitly published. Source directories
are never the execution working directory. Apply/merge results into the source
project as a separate, reviewed local operation.

Only task input, necessary summaries, dependency results, workflow checkpoints,
observations and artifact references cross the boundary. Complete Session,
Context and Memory are not synchronized. Published reports and logs may contain
sensitive task data and are visible to approved instances of the same project.

## Failure semantics and limits

The control plane fences reports and uploads by instance, incarnation, task
generation and unexpired lease. Restarting a worker changes incarnation. Old
workers cannot renew expired leases or overwrite a reassigned task. Duplicate
submissions/reports/uploads are idempotent; conflicting repeated IDs are rejected.
During connection loss, a worker stops local execution before its last acknowledged
lease deadline; Crew later retries eligible work. Interrupted executions restart
from the checkpoint/task input in a new workspace.

Execution is at least once. Fencing protects authoritative results, not external
side effects that happened before a crash; make external mutations idempotent.
CPU/RAM/GPU values are scheduling reservations, not OS quotas or automatic GPU
device assignment. Artifacts are limited to 8 MiB each. There is no automatic
artifact/workspace garbage collection, object storage or multi-region consensus.
The current control plane is one authoritative AXCrew process backed by SQLite.
Use durable storage and backups. Physical multi-host deployment and hardware
resource isolation require deployment-level verification.

See [AXCrew's control-plane contract](../../axcrew/docs/distributed-collaboration.md)
and [ADR 0021](adr/0021-durable-distributed-collaboration.md).
