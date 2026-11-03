# AX-Bench Phase 1: Runtime Microbenchmark

An independent, offline Rust benchmark package plus a Python standard-library
runner/reporter. It links the actual `runtime-core`, `memory`, `model` and `tool`
crates through their public APIs. No production source or root workspace target
depends on this package. No Agent ability evaluation or other-harness comparison.

## Run

From the AX repository, with Rust 1.92+, Python 3.10+ and dependencies already
cached (offline builds fail rather than downloading missing dependencies):

```powershell
python benchmarks/runtime/run.py
python benchmarks/runtime/run.py --rounds 3 --warmup 5 --samples 20 --sizes 16 128 1024 --output benchmarks/runtime/results/my-run
```

The runner first builds the current CLI and harness with `--release --offline
--locked`; compilation is never timed. The independent lockfile was seeded from
AX's lockfile, retaining shared dependency versions. Keep both lockfiles when
archiving a baseline. Tokio has four worker threads. No third-party benchmarking
or Python dependencies are introduced.

The Rust code is a custom benchmark harness, also registered as Cargo's `micro`
benchmark (`harness=false`); `cargo bench --manifest-path
benchmarks/runtime/Cargo.toml --bench micro --no-run --offline --locked` checks
that target. `run.py` executes the same code as a binary to control isolation,
round boundaries, resources and output. Direct worker invocation requires an
empty disposable cwd and `AX_BENCH_WORKDIR` equal to that cwd, to prevent writing
fixtures into a user's project. Use the unified runner for ordinary measurements.

## Production paths and units

- Startup: real `ax --version`, `ax --help`, independently launched for **each**
  sample; process cold, OS cache uncontrolled. Includes loader, Tokio main setup,
  argument parsing, exit and parent launch/wait overhead. Not full agent startup.
- Kernel init: `AgentKernel::new` and its baseline-tool wiring; one construction
  per operation. Excludes process and Tokio runtime creation.
- Session: `MemoryStore::create_session`; `session` + `load_messages` (full
  JSONL-backed history); `effective_context` + `load_context_page` +
  `select_context` (local resume components). Snapshot load adds actual
  `save_effective_context` setup, JSON deserialization and watermark checks.
  The no-snapshot case selects fixed model messages matching the stored fixture;
  it does not execute private CLI message conversion or full resume.
- Context: `select_context` at 8192 total / 1024 system budget,
  `estimate_tokens`, and serde serialization of actual model messages. No
  model compression. Input alternates user/assistant messages from fixed UTF-8
  `datasets/message.txt`, repeated at 16/128/1024 messages.
- Memory: production SQLite/JSONL `MemoryStore`, `memory_index`,
  `retrieve_index` (query `build command runtime`, fixed timestamp 1800000000),
  and `remember_scoped` fixed-key upsert with n indexed project records. Seeding
  is untimed. Size for upsert means database population, not batch writes.
- Tool: `ToolRegistry::register`, `get`; real kernel tool-round scheduler with
  n calls to an immediate deterministic Tool. Registration includes constructing
  names/mock objects and dropping the registry. Dispatch includes two mock model
  responses, request assembly, permission checks, events and result handling;
  asserts that all n tool bodies actually executed. Scheduler bound is the
  production default four; dispatch size counts tool calls, not model turns.
- Subagent: actual `prepare_subagents` → `spawn_agent` → `wait_agent`, using
  real child fork/run/coordination with a deterministic `ChildHost` and no-op
  persistence checkpoints. One child per fresh parent. This excludes real CLI
  workspace/Git/session provisioning and durable child storage.
- Concurrency: actual `AgentSupervisor::run_tasks`, 16/64 tasks with concurrency
  1/4/8. Each forks a fresh kernel with a yielding fixed-response provider.
  Operations count completed tasks. Template/task construction is untimed.
- Boundary baselines: direct deterministic `ModelProvider::complete` and
  `Tool::execute`, separately labeled; never subtract these from combined times.

Memory read/recall and session operations count one API sequence per operation;
context and lookup count one call; tool registration/dispatch and concurrency
count n items per sequence. Every raw sample contains elapsed ns and its exact
operation count. `size` is meaningful only for the scaled cases above.

## Sampling and reproducibility

Default: three independent worker processes **per case/size/concurrency**, each
with five untimed warmup samples and twenty recorded samples. Context and tool
lookup batch 100 calls per sample; other cases use one sequence. Reported P95 is
nearest-rank over batch-normalized ns/op, not individual-call tail latency.
Throughput uses total operations / total timed duration. Correctness assertions
abort failed workers; failures are retained and make the runner exit nonzero.

Each worker gets a new temporary cwd, DB, event files and `AX_HOME`, then cleanup.
Read fixtures are seeded before timing. Session creation is deleted after each
timed call; memory upsert replaces one fixed record, avoiding database growth.
No real provider, network tool, shell, MCP server or credentials are used.
The runtime may perform its normal local environment/executable probes.

CPU and memory telemetry is **whole-worker**, including setup, warmup and
teardown; it is not per-operation allocation. Windows uses GetProcessTimes and
GetProcessMemoryInfo (working set / peak working set); Linux samples /proc
(short-lived peaks/CPU may be missed); macOS telemetry is explicitly unavailable.
Sampling runs on a separate parent thread at 2 ms. CPU % is relative to one
logical core, and may exceed 100%; missing measurements are null. These short
runs establish latency and completion baselines, not leak/soak stability.

Machine load, power policy, CPU clocks, filesystem cache and antivirus remain
uncontrolled. Run benchmarks after builds/tests finish, using a consistent host
and power mode. Do not infer version regressions from one noisy result.

## Outputs and cross-version comparison

`results/<UTC timestamp>/` (ignored by Git) contains:

- `raw.json`: schema/fixture versions, commit and tracked dirty state/diff hash,
  production-source hashes, Rust/OS/CPU/RAM, flags, parameters, fixture/lockfile/
  executable hashes, per-round raw samples and worker resource counters.
- `samples.csv`: one row per raw latency sample with its worker resource scope.
- `summary.json`: aggregate latency and throughput for each case/configuration.
- `report.md`: environment, Mean/Median/P95/Min/Max, throughput, resource
  telemetry per round, boundary labels, failures and limitations.

```powershell
python benchmarks/runtime/compare.py path/to/old/raw.json path/to/new/raw.json --output benchmarks/runtime/results/comparison
```

Comparison emits JSON/CSV/Markdown descriptive changes, warning on environment,
parameter and case-set differences. It imposes no significance test or CI gate.
Archive source revisions/patches separately for dirty baselines: hashes identify
content but do not reconstruct it. Preserve this harness revision when comparing
production versions; changing fixtures, unit definitions or timed boundaries
requires a new fixture/schema version. Paths to results in reports are local
artifacts, not public test dependencies. Private regression tests remain in
`test/`; this directory contains reusable benchmark infrastructure only.

Phase 1 does not cover model/API performance, real external tools, private CLI
memory injection, full durable resume, model-based compression, real child
workspace provisioning, crash recovery, long-duration stability, Agent Task
Success Rate or useful-work throughput. These limitations also appear in each
generated report.
