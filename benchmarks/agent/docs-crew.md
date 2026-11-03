# Real distributed execution and fault orchestration

`crew.py` is an external controller for the production Crew REST API and real
AX workers. It has no simulated-node execution mode. Controller regression
fixtures are not distributed benchmark results. No production AX/Crew code is
modified. Use a dedicated server/projects; existing user data is not reset.

## Entry points and repetitions

```powershell
python benchmarks/agent/crew.py --config C:/private/crew-bench.json --output benchmarks/agent/results/crew-plan
python benchmarks/agent/crew.py --config C:/private/crew-bench.json --execute --output benchmarks/agent/results/crew-live
```

Without `--execute`, no HTTP request, node command or model call occurs. Each
matrix entry has at least three repetitions. The callable interface is:

```python
run_experiment(config, output, execute=False, round_numbers=None)
```

It returns the complete report (`metadata`, `rounds`, `summary`). A parent Runner
that already guarantees three repetitions may pass `round_numbers=[outer_repeat]`
to avoid multiplying real runs. The report labels that repetition scope explicitly.

## Tasks, DAGs and deterministic verification

Every task has a safe unique `id`, a production `submit` object and one or more
of `expected_result_json`, `artifact_checks`, `verify_command`. `depends_on`
contains task aliases, not server IDs. The controller rejects unknown aliases,
cycles and pre-existing workflow/parent references, namespaces request ids per
repetition, binds a native Workflow per project/group/copy and translates the DAG
to real server task dependencies. The production AX worker receives dependency
results and downloads/applies the native immutable artifacts. No alternative
agent scheduler is introduced.

The first workflow root is submitted before siblings to bind its real Workflow
identity. Sibling submissions use bounded parallel HTTP. Crew independently owns
execution concurrency and configured worker slots. The controller records both
configured submission concurrency and sampled actual concurrent running work.

`scalability` is a list of `{nodes:[node_id,...],concurrency:N,copies:N}`. Copies
repeat the full task DAG in separate Workflows. Selected registered AX instances
become the Workflow participants; other workers are not silently used. Empty
`nodes` supports already-provisioned dedicated clusters, but cannot attribute
node resources or assert a requested node count. Different matrix entries test
node/workload counts, not separate harness/model experiment groups.

`expected_result_json` compares the exact final task JSON. `artifact_checks`
selects immutable artifacts by optional `kind` and `name`, and may check
`sha256`, `expected_json`, or `expected_text`. A selector alone verifies presence
and downloaded content integrity, not semantic correctness. Every artifact is
size/SHA256 checked, stored under a hash-derived controller filename, and listed
in `artifacts.json`; server filenames never become filesystem paths. For richer
checks, `verify_command` is a trusted local argv array using `{artifacts}` and
`{task_id}` placeholders. It must exit zero and emit JSON `{"passed":true}`.
The verifier must execute untrusted patches in its own isolated environment;
the controller never applies or imports candidate patches into its source tree.

The example DAG transfers a ledger patch between real worker workspaces. Its
small arithmetic result is only an example; it does not establish complex
collaboration quality. Provide substantive repository-specific tasks/verifiers.

## Owned worker lifecycle

`nodes` entries contain `id`, `instance_id`, `transport` (`local` or `ssh`),
`dedicated_benchmark_environment:true`, optional `ssh_host`, `run_directory`,
and `commands`. Every command is an argv array. SSH uses BatchMode and POSIX
argument quoting; passwords are not handled. The remote machine must have the
selected commands, benchmark scripts and production AX installed.

Commands may include `prepare`, `start`, `identity`, `probe`, `stop`, `crash`,
`disconnect`, `reconnect`, `cleanup`. Every mutating command must contain the
`{run_id}` placeholder; prepare/start requires explicit cleanup. Placeholders
`{run_id}`, `{run_dir}`, `{node_id}` are substituted as argv values. The runner
never guesses a PID, kills an installed app by name, edits a global firewall, or
provisions a cloud host. Commands and output are bounded and retained privately;
reports contain command digests rather than raw credentials/argv.

`worker_control.py` supplies real start/stop/crash/cleanup/identity/probe commands:

```text
python /bench/worker_control.py start --run-id {run_id} --run-dir {run_dir} --binary /opt/ax --worker-template /private/benchmark-worker.json
python /bench/worker_control.py crash --run-id {run_id} --run-dir {run_dir}
python /bench/worker_control.py start --run-id {run_id} --run-dir {run_dir}
python /bench/worker_control.py probe --run-id {run_id} --run-dir {run_dir}
python /bench/worker_control.py cleanup --run-id {run_id} --run-dir {run_dir}
```

The template uses the production WorkerSettings format (gateway, token,
instance_id, projects, ax_home, provider/model, etc.). It must name a dedicated
AX Home; only its config/auth/provider metadata is copied into the new run Home,
not existing sessions/memory. Execution roots are rebased under the exact run
directory. PID files retain a process birth identity; it must still match before
any signal. The run directory must have the exact run_id as a path component and
an owner marker. Windows tree stop is abrupt; Unix stop tries TERM before KILL.
Cleanup stops only the owned worker and preserves all configuration/evidence.
The private worker configuration contains its token; never publish run directories
wholesale. Linux namespace/container jobs should include an init/reaper.

Network faults use explicit disconnect/reconnect scripts for the owned container
or isolated network namespace. Those scripts must operate on the supplied run_id;
there is no portable host-wide network-disconnect default. This is an environment
integration point, not a simulated network error or an unimplemented controller.

## Controlled faults and assertions

`faults` entries specify node id, `kind` (`crash`, `stop`, `disconnect`),
`after_seconds`, `hold_seconds`, optional task alias, `require_running` (default
true), and `require_reassignment`. Injection is recorded only after the fault
command succeeds. The captured task owner/incarnation/generation identifies the
actual affected attempt. Restoration is scheduled during polling and retried
in cleanup even if injection/measurement fails or the user interrupts. A fault
that never triggers does not receive a recovery pass.

Recovery requires the affected task to complete its deterministic verifier and
advance to a new attempt. Optional reassignment also requires another instance.
Recovery time uses actual attempt completion timestamps relative to injection;
server wall-clock skew limits cross-host timing precision. CPU/RSS and wall time
include controller-visible setup/verification/cleanup scopes as labeled.
Pre-fault published artifact hashes are retained and compared afterwards. A
missing/changed published artifact is recorded as data loss and prevents a recovery
pass. If no artifact existed before injection, data loss remains unmeasured;
raw Session filesystem loss needs an independent node verifier. Attempt/session
history retention is also checked across observed snapshots.

Default idempotency assertions replay each identical submit and require the same
task id, then submit conflicting content under the same id and require rejection.
The controller retains/cancels any unexpected new task created by a faulty server.
Consistency checks inspect monotonic cluster revision/event sequence, retained
submitted tasks, increasing attempt generations, owner/incarnation alignment,
terminal-result immutability and dependency execution order. They cover observed
snapshots, not a mathematical proof of all distributed histories.

With node `worker_token_env` configured, after an actual attempt replacement the
controller authenticates the old worker identity and sends a deliberately stale
**observation** (never completion/failure). Generation fencing must reject it.
An authentication error alone is not a fencing pass. `require_fencing:true`
requires such observed checks, so an untriggered replacement is not scored as
fencing success. Tokens come from environment variables and are never printed.

`check_checkpoint_cas:true` performs a native Workflow checkpoint write with its
current revision, then requires an identical old-revision write to be rejected.
It preserves existing state fields and adds only a namespaced benchmark probe.
Concurrent worker checkpoints are handled with three bounded fresh-read retries.
`check_receipt_idempotency:true` tests real running-worker observation receipts:
identical messages must replay, conflicting message content must be rejected,
and the observation must appear exactly once in durable events. If no running
attempt/token is observed, this configured assertion cannot pass.

## Resources, budgets and scope

`probe` commands must emit JSON with the current `run_id` and measured fields:
`process_identity`, cumulative `cpu_seconds`, `rss_bytes`, cumulative
`network_bytes`, optional `input_tokens`, `output_tokens`, `api_requests`,
`cost_microusd`, `usage_identity`, `usage_estimated`, `cost_estimated`. Missing
measurements remain null. `resource_probe.py --pid-file FILE --run-id ID` can
collect real worker PID counters; it refuses a PID file from another run.
Windows reports CPU/RSS and leaves network unavailable. Linux network counters
cover the process network namespace, including other processes in it. Use a
private namespace for attribution. CPU/RSS probes cover the named process, not
all executed tools; configure an external cgroup/job collector for full-node data.

The aggregator differences cumulative CPU/network samples instead of summing
snapshots. It retains process counter resets and sums per-node sampled RSS maxima
(which are not necessarily simultaneous). Short tasks/unsampled exited children
may be missed. Built-in event traces report Goal-cumulative usage that can include
imported node checkpoints: they are conservatively labeled estimated to prevent
cross-node double-counting from being presented as billed usage. A collector
providing unique request-level billing can report exact usage instead.
Linux `network_identity` combines host boot identity and network-namespace inode.
A worker restart in the same namespace does not restart network lifetime counters;
workers sharing a namespace are de-duplicated. Missing namespace identity across
process restarts or multiple nodes makes aggregate network bytes unavailable,
with known observed deltas retained separately as partial evidence.

`global_seconds`, `global_tokens`, `global_cost_microusd`, `max_cpu_seconds`,
`max_rss_bytes`, `max_network_bytes` use zero for unlimited. Finite limits with
missing or estimated corresponding measurements fail closed: owned live tasks
are cancelled and later rounds are not admitted. An in-flight request may exceed
a cap before observation. This is not a prepaid invoice/cloud billing guarantee.
Native AX task budgets can also be supplied in the actual task Goal configuration.

`task_budget` (global defaults or per task overrides) wraps the original task
target in the existing native `/goal create` command with max_rounds,
max_seconds, max_tokens and max_cost_microusd. The production worker's durable
Goal checkpoint path carries that policy through real takeover; no Runtime
budget implementation is replaced. Native caps retain their documented complete
usage/billing prerequisites and in-flight overshoot. The controller refuses to
nest this wrapper around an already explicit `/goal` command.

Controller/Crew HTTP request counts, payload bytes and request elapsed time are
measured separately as coordination overhead. They exclude headers/TLS and
worker-to-server traffic. Resource probes are not fabricated from reservations
or node counts.

Reports distinguish actual single-host workers, multiple logical Host IDs with
unknown physical identity, and operator-attested cross-machine execution.
Physical `machine_fingerprint` values in `inventory` are correlated with observed
attempt Host IDs; fingerprints/identities remain operator attestation. Reports
also retain actual attempt models, optional run-scoped node identity outputs,
configured source revisions (unverified declarations unless identity commands
check them), config hashes, raw snapshots, fault commands/receipts, verifier logs
and cleanup errors. Atomic JSON/CSV/Markdown reports survive interruption.
