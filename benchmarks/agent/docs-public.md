# Production AX on public benchmarks

`public_execution.py` executes imported tasks with the production **Linux AX
binary**, not an alternate agent. It creates a fresh bounded Docker container,
AX Home, workspace and persisted session for every task/profile/repeat. At least
three repeats are required for capability runs. A separately labelled cost
calibration may use exactly one task, one profile and `rounds=1` by explicitly
setting `calibration=true`; it is not a stability or leaderboard result. It does not build/pull images or provision servers;
prepare the official task environments locally before execution.

```powershell
python benchmarks/agent/public.py import-swe --source C:/datasets/verified.jsonl --revision PINNED_DATASET_REVISION --output C:/private/swe-manifest.json
python benchmarks/agent/public.py import-terminal --source C:/datasets/terminal-bench --revision PINNED_DATASET_COMMIT --output C:/private/terminal-manifest.json
python benchmarks/agent/public_execution.py --config C:/private/public-bench.json --output benchmarks/agent/results/public-plan
python benchmarks/agent/public_execution.py --config C:/private/public-bench.json --execute --output benchmarks/agent/results/public-run
```

The command without `--execute` generates JSON/CSV/Markdown planning reports and
never inspects Docker, starts AX or calls a Provider. The example configuration
contains replacement markers; provide one prepared environment mapping for each
manifest task. Pin images by local `sha256:` ID or repository `@sha256:` digest.
Mutable image tags are rejected and container creation uses `--pull=never`.
Use a dedicated Linux-compatible AX Home template and record the AX build commit
with its binary hash. An existing executable is hashed/version-checked; its
correspondence with the recorded working-tree source is not inferred.

Build `public-probe/` for the same Linux target, from the same source revision as
AX, and set `sandbox_probe_binary` to that executable. On a prepared Linux builder:

```bash
cargo build --release --locked --offline --manifest-path benchmarks/agent/public-probe/Cargo.toml --target x86_64-unknown-linux-musl
```

The probe links the production sandbox crate and actually prepares its workspace
policy, starts the native broker and executes `/bin/true` through it. It makes no
model request. Missing bubblewrap/user namespaces, incompatible Docker seccomp,
or any failed backend preparation is `not_run` before inference. The driver never
retries with `--sandbox off`, `--privileged` or added agent capabilities.
An explicitly vetted Docker seccomp file can be configured with `seccomp_profile`
and its exact `seccomp_sha256`; changed or default-allow policies are rejected.
Agent containers still drop capabilities and enforce no-new-privileges. Images
declaring a `VOLUME` overlapping the candidate workspace/AX data are rejected,
since Docker snapshots do not retain anonymous-volume changes.

The runner copies only the configured production binary, task instruction and
private AX configuration/credentials into the agent container. Hidden verifiers
and reference solutions are not copied into that container. Agent images must
already contain task inputs and dependencies. The importer records every task
source hash and refuses modified/symlinked Terminal source trees at execution.
Only a prompt, repository/base commit and immutable source metadata are retained
from SWE dataset rows; gold and test patches stay controller-only.

## SWE-bench Verified

Prepare the benchmark checkout at its `base_commit` inside the configured agent
image. The runner verifies that HEAD before any model request. After execution,
it exports the actual diff against that base, including newly created files and
excluding its own `.ax` Goal settings. An agent-created commit does not hide the
diff: extraction always uses the dataset base. AX is stopped before official
evaluation. Production trajectories, usage, patch and session data are retained.

Configure `swe_evaluator` with an unchanged upstream source checkout at a full Git
commit and a Python environment containing its pinned dependencies. The local
original dataset must match the imported SHA256. Current upstream evaluators use
prepared task metadata containing `eval_script`, `log_parser`, `eval_type`, image,
and test lists; generate that metadata with the same pinned upstream preparation
workflow. Supply its file SHA256. The driver checks that instance identity,
repository/base, held-out test patch and test lists still match the original
row. Map `evaluation_image` to the locally prepared official image digest; that
exact image is inspected before model execution and passed to the evaluator.
No gold patch is applied by the Agent driver.

The driver invokes the upstream `swebench.harness.run_evaluation` module with
one prediction and an exact instance id. It ingests only that instance's boolean
`resolved` field from the fresh official `report.json`; an aggregate success
message or process exit alone never produces a pass. If evaluation fails or no
instance report exists, the attempt is `ungraded`, with a null official score.

The unchanged upstream module runs through a small Docker-creation wrapper that
limits only this trial's unique verifier name to the configured CPU, memory,
process and network bounds. Image pulls/builds and unrelated container creation
are refused. The wrapper does not alter test scripts, patches or score logic.
Evaluator imports and prepared images are checked before any model request.

The original and prepared dataset digests, source commit, evaluation image,
prediction digest, official report digest, unique run id and model ids are
recorded. The official verifier executes the real repository tests, without a
second agent or comparison experiment. See the
[official evaluator](https://github.com/SWE-bench/SWE-bench/blob/main/swebench/harness/run_evaluation.py)
and [prepared task contract](https://github.com/SWE-bench/SWE-bench/blob/main/swebench/harness/utils.py).

## Terminal-Bench legacy and version 2

The importer handles legacy `task.yaml` instructions and the standard version 2
`instruction.md` / `task.toml` / `environment/` / `tests/test.sh` layout.
Legacy YAML parsing requires PyYAML; it does not grade task success. Unsupported
or incomplete task layouts produce a concrete prerequisite error. Multi-container
or alternate environment specifications must be materialized into the prepared
image(s)/network before invoking this single-agent-container executor. Additional
multi-step Harbor formats are not silently interpreted as a single instruction.

After AX exits, credentials are removed, its filesystem is snapshotted, and the
original agent container is stopped. Official tests run in a **new verifier
container** from that snapshot. Candidate background processes cannot watch the
hidden tests being copied in. Stale verifier state is cleared before copying the
unchanged `tests/` tree. Reference `solution/` or `solution.sh` files are never
copied. Both agent and verifier receive CPU/memory/process limits.

For version 2, the original `/tests/test.sh` writes its official numeric reward.
The driver reads `reward.txt`, or a declared `reward_key` in `reward.json`, under
`/logs/verifier`. Missing, ambiguous or nonfinite rewards are ungraded; successful
reward plus a failed verifier process is also ungraded. For legacy tasks, the
unchanged `run-tests.sh` runs with `TEST_DIR=/tests`, and a pinned upstream
`ParserFactory` parses its actual output. All returned cases must pass; an empty
or missing case set is ungraded. This follows the
[version 2 task/reward format](https://docs.harborframework.com/tasks/overview)
and [legacy parser interface](https://github.com/laude-institute/terminal-bench/blob/main/terminal_bench/parsers/base_parser.py).

## Budgets, evidence and cleanup

Finite positive task/verifier time, steps, tool calls, CPU, memory and process
limits are mandatory. Task token/cost limits use native Goal accounting; global
admission checks actual reports and stops later tasks when a requested finite
cap cannot be metered reliably. An in-flight request may cross a cap. No cloud
server is started by the driver. Verifier time is counted in the global deadline.

Each attempt has a private directory containing actual Docker command output,
Agent events/session data, candidate patch, official test output and score,
source/image/model identities and cleanup errors. Authentication templates remain
private local material and must not be uploaded wholesale. Secrets are never
printed into the report metadata. `env_from` passes named environment variables
to Docker without putting their values in command arguments.

Successful containers and temporary verifier snapshots are removed. Failed or
ungraded agent containers are stopped and retained by default for investigation,
after their credential directory is removed. If credential removal fails, the
container is removed instead. Cleanup touches only unique names owned by that
attempt, including its explicitly named official SWE verifier. Cleanup failures
are recorded and require operator attention. Set `retain_failed_containers=false`
to remove all trial containers while keeping the captured evidence.

Shared report statistics distinguish executed but ungraded attempts from scored
attempts. Missing official tests are never relabeled as task correctness failures
or successes. Docker-client process resource readings are not attributed to AX;
container/remote CPU, memory and network metrics remain null unless collected
separately. Public executions are real once prerequisites/model are supplied;
controller regression fixtures and offline plans are not public benchmark scores.

## Single-task cost calibration

Set `calibration=true`, `rounds=1`, and provide exactly one task and one profile.
`profiles[].reasoning_effort` passes the existing production CLI setting and is
recorded in provenance (for example `low`). Normal benchmark mode still requires
at least three repeats. Calibration retains the same original task, verifier,
workspace isolation, trajectory and budget rules; only sampling differs. Account
balance deltas can include other simultaneous account activity, so compare them
with the retained Provider usage rather than treating the delta as exact task
billing without that qualification.
## Public container setup options

`container_uid` / `container_gid` select the prepared image's execution identity.
Trusted input uploads use matching numeric archive ownership so a restricted
container does not need CHOWN/FOWNER merely to read AX or its private Home.
Prepare workspace ownership for that identity; never change task content to make
a solution pass. Cross-platform task fingerprints use portable POSIX keys.

For production bubblewrap on Docker, a non-root container can explicitly request
`nested_userns_systempaths=true` together with a pinned default-deny seccomp
profile. Docker otherwise mounts `/proc/sys` read-only, blocking bubblewrap's
user-namespace-scoped `max_user_namespaces` setup. This enables Docker's
`systempaths=unconfined` option only for the non-root agent container; it does not
make seccomp unconfined, add agent capabilities, enable privileged mode or skip
AX's production sandbox preflight. It relaxes the outer system-path masks and is
an explicit environment choice recorded in provenance, not a security benchmark
configuration. The independent adversarial security executor is unchanged.

Original trusted verifier setup scripts may require package-install capabilities.
`verifier_capabilities` explicitly permits only CHOWN, DAC_OVERRIDE, FOWNER,
SETUID and SETGID for the separate credential-free verifier container. They never
reach the agent container. Set per-task `required_verifier_files` such as
`["ctrf.json"]` to avoid interpreting a zero reward from failed verifier
installation as an observed task-test failure. Keep environment initialization
failure distinct from candidate assertion failure.
