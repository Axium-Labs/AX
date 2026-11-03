# AX-Bench Agent Capability and Advanced Benchmark

This package implements the Phase 2/3 benchmark controllers around production AX
and AXCrew. It reuses Phase 1's resource sampling and timing statistics. It does
not change either product's Runtime or inject predetermined model answers.
Python controllers require Python 3.10+. Native AX builds use the repository's
Rust toolchain. Container and upstream official validators have their own pinned
prerequisites described below. **No model is called without `--execute`.**

## Task suites and execution

The standard suite has 60 custom deterministic tasks: 20 coding algorithms,
10 file-processing/discovery tasks, 5 session-resume and 5 cross-session memory
tasks, 10 defective-program repairs, 5 multi-step algorithm tasks and 5 MCP tasks.
Repairing a defective program is not counted as API fault recovery. MCP fixtures
are real stdio services; AX's model, client and execution remain production paths.

The Advanced catalog has 24 concrete scenarios with executable dispatch:

- Ledger reconciliation at three sizes; multi-process context recall with actual
  compression assertions; a configurable staged pipeline crossing Sessions;
  automatic rotation with original-fact retention and observed rotation events.
- Process crash/resume with pre-crash workspace hashes and committed JSONL-prefix
  receipts; transient MCP failure; API 429/503/nonresponse and connection reset.
- Native token, cost, time and round-budget stops, stagnation and advisory loop
  detection. Verifiers inspect native Goal reasons/states and persisted advice.
- Five isolated security vectors: permission boundary, sandbox escape, untrusted
  prompt injection, synthetic-secret exfiltration and destructive-command denial.
- A real Crew experiment dispatch: DAG tasks, explicit node-control/fault hooks,
  consistency/fencing checks, artifact checks and node resource collectors.

Environment availability is checked by executors. Missing Docker, a prepared
image, a supported endpoint binding or a dedicated node configuration yields
`not_run` with a concrete reason. These are no longer catalog placeholders.
Controller unit-test fixtures are not model/Agent capability measurements.

The staged pipeline uses `advanced.stages` (2–10000, default 20); every third stage
starts a new Session and verifies prior durable output. `advanced.minimum_seconds`
can require an observed duration for a soak campaign. Raising a fixture size or
forcing a minimum duration is not itself evidence that a task completed reliably.
Set a suitable finite `task_seconds` for long campaigns.

## Plan offline

From the AX repository root:

```powershell
python benchmarks/agent/run.py --suite all --output benchmarks/agent/results/plan
python benchmarks/agent/crew.py --config benchmarks/agent/crew.example.json --output benchmarks/agent/results/crew-plan
```

Plans schedule at least three repeats and generate JSON/CSV/Markdown reports with
zero attempted tasks and null success rates. They never inspect/start a model.
Existing result directories are not overwritten. A separate executable or image
must be prepared for the relevant host/container platform before execution.

## Select a model and execute

Copy `config.example.json` privately and set the Provider id, model id and model
snapshot revision where exposed. Multiple profiles are supported; they are
reported separately, with no alternate harness or model control experiment.
Actual `ModelStarted` identities are retained separately from declared ids.

Native tasks need an existing dedicated `home_template` directory. Only
`config.json`, `auth.json` and `providers.json` are copied; original sessions,
memories, hooks, skills and permissions databases are not imported. Every
attempt has its own Home, workspace, data store and Session. Multi-turn tasks
reuse only their own state; memory tasks explicitly cross Session boundaries.

```powershell
python benchmarks/agent/run.py --config C:/private/axbench.json --execute --task stable_unique --rounds 3
python benchmarks/agent/run.py --config C:/private/axbench.json --execute --suite standard
python benchmarks/agent/run.py --config C:/private/axbench.json --execute --suite advanced
```

Native execution builds current production Release AX offline/locked before
measurement. `--skip-build --binary PATH` records an external binary digest and
marks its correspondence with current source unverified. Security-only and
Crew-only dispatches do not require or build the host executable.

The example explicitly sets `allow_dangerous=true`, mapping to AX's existing
flag, so edits, shell commands and MCP discovery can run unattended within the
required workspace sandbox. `false` measures AX's safe-only policy instead.
The runner never escalates this setting after a failure. Security tasks have
their own isolated execution and explicit deny rules. Unsupported sandbox
backends fail closed; no task automatically retries with confinement disabled.
Ordinary local subagent depth is explicitly zero to bound accounting; Crew tests
exercise real independent workers through their own configuration.

## Fault injection and native constraints

For API fault scenarios set `profiles[].fault_gateway.upstream` to an HTTPS
**origin**, without path, credentials or query. The runner starts a loopback-only
gateway, temporarily binds the production endpoint environment variable and
restores it afterward. Supported existing endpoint overrides are OpenAI API-key
Responses, Anthropic, Google, Radius and Azure Responses. Built-in endpoints
without an override (such as Codex OAuth) are reported unavailable for this
in-process gateway; no product routing code is changed for the benchmark.
`endpoint_path` optionally overrides the protocol-specific local path.

Only the configured initial requests receive transport faults; subsequent traffic
is forwarded to the real TLS-verified Provider. Request bodies, authorization and
URLs containing credentials are not logged. Recovery requires actual injection,
a later successful real upstream response, correct artifacts and completion.
Gateway network bytes cover HTTP payload only, not TLS/TCP overhead or tools.

The nonresponse scenario holds a real request beyond a five-second native idle
turn deadline, then resumes the saved Goal/Session after the fault. It measures
cancellation/recovery from an unresponsive Provider under AX's native deadline;
it does not claim that every Provider has an independent per-request timeout.
The connection-reset scenario tests transport reconnection, not physical NIC or
whole-machine network isolation. Crew node hooks separately support explicit
network disconnect/reconnect scripts on dedicated nodes.

Native budget assertions require actual Goal stop reasons and states. An outer
watchdog timeout cannot pass them. Token/billing tests without complete reports,
or loop tests where repeated calls were never exercised, are `unscorable` and
retain their execution cost/evidence while being excluded from score denominators.
A stopped reliability scenario can pass its constraint criterion without being
misrepresented as an ordinarily completed user task; category and trace remain.

## Isolated security

[Security executor and environment configuration](docs-security.md) describes the
standalone command and `config.security`. All attack code runs inside a pinned,
non-root, read-only, capability-restricted disposable Linux container. There are
no host bind mounts or real user credentials. A dedicated inspected internal
network reaches only an operator-provisioned allowlisted model gateway, which
holds real Provider credentials outside the attack container. The executor
checks image/binary/revision identities and applied isolation before inference.
Synthetic canaries and explicit deny rules permit deterministic assertions.

Defensive model refusal is distinct from a typed Runtime interception. No tool
attempt means no interception-rate denominator. Sandbox errors never trigger a
privileged retry or a host execution fallback. Code/controller verification on a
host without Docker does not constitute a security benchmark score.

## Real distributed Crew experiments

[Distributed orchestration and collectors](docs-crew.md) covers `crew.py`,
`node_control.py`, `resource_probe.py` and `crew.example.json`. Configure a
dedicated server, registered worker instances/projects, explicit run-scoped
local/SSH lifecycle/fault commands and read-only resource/usage probes. Existing
Crew scheduling, dependencies, artifacts, leases and generation fencing do the
actual work. No simulated workers or cloud machines are silently created.

The standalone CLI runs >=3 repeats. In the main Runner, set `crew` to a private
Crew config file path or embedded object; the parent passes its own repetition
number so three repeats do not accidentally become nine. Crew model bindings
must match the selected profile. Multiple scalability cases can vary actual node
selection, submission concurrency and task copies. Per-case traces and metrics
remain separate. Unknown remote usage stays null and finite budgets fail closed.

Logical Host IDs alone are not proof of physical distribution. Reports distinguish
single-host execution, multiple logical Hosts and operator-attested machine
fingerprints correlated with observed attempts. Resource counters retain their
PID/process/network-namespace scope; namespace-wide traffic is not called PID-only
traffic. Explicit faults are acknowledged and restored in cleanup. Missing fault
triggers do not become successful recovery observations.

## Official public datasets

[Public dataset execution and official validators](docs-public.md) describes
`public.py`, `public_execution.py`, prepared container images, the production
sandbox probe and upstream validator prerequisites. SWE-bench Verified metadata
is imported without gold patches in agent-facing instructions; Terminal-Bench
legacy and v2 task layouts are supported. Sources, dataset revision, images,
evaluator checkouts and binaries are pinned and checked.

```powershell
python benchmarks/agent/public_execution.py --config C:/private/public-bench.json --output benchmarks/agent/results/public-plan
python benchmarks/agent/public_execution.py --config C:/private/public-bench.json --execute --output benchmarks/agent/results/public-run
```

AX runs in the prepared task environment. Hidden Terminal tests are copied only
after AX exits, into a fresh verifier container. SWE predictions are passed to
the pinned official evaluator. Scores require exact-instance official reports,
upstream parsed tests or explicit upstream rewards. Missing validation is
`ungraded`, never a guessed success/failure. Environments are prepared explicitly;
no image pulls, cloud provisioning or large dataset downloads occur automatically.
Custom tasks are never labeled official leaderboard results.

## Budgets and evidence

Time, step and tool-call limits must be positive and finite. `0` means explicitly
unlimited for token/cost limits. Global admission stops at measured consumption,
or fails closed when requested finite caps lack complete metering. In-flight
requests can cross a limit: this is not a prepaid billing guarantee. Native hard
caps require complete Provider reports; estimates are visibly labeled and cannot
supply hard admission. Failed/ungraded attempts still contribute measured spend.

The native supervisor retains stdout/stderr and kills its owned process group/tree
on timeout or interruption. Detached descendants need external OS/container
supervision. Candidate function checks use separate bounded processes; they are
ordinary correctness tests, not the adversarial security isolation boundary.
Security/remote/public adapters have their own bounded cleanup and resource scopes.

Reports include `results.json`, `runs.csv` and `report.md`, updated atomically after
attempts and on interruption. They retain failed workspaces, Session JSONL/index,
turn traces, verifier logs, faults, node observations and available evidence hashes.
Native crash tests compare complete JSONL prefixes captured before interruption;
this is an explicit measured subset, not a proof of all Session durability.

Git HEAD, dirty status, source/diff/binary/lock/config hashes, dataset versions,
model ids/revisions and OS/Python identify the run. Local Home copies contain
credentials: result directories are private/ignored and must not be uploaded
wholesale. Dirty-tree results are never renamed as clean-commit baselines.

Success/coverage, nearest-rank P95 and mean/median/min/max durations, throughput,
real tool outcomes/repetitions, usage/cost, recovery, security assertions and
failure reasons are reported with descriptive Wilson 95% intervals. Per-task,
category and model statistics remain available. Repeats share task difficulty;
intervals do not establish independent population sampling. Missing measurements
remain null. Tool semantic selection/argument accuracy, subjective quality and
parallel-efficiency scores need independent task-specific annotations; no guessed
score or invalid-call classification is derived from repetition alone.
