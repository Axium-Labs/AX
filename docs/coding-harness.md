# Coding execution harness

AX enables the coding harness at the shared CLI runtime composition boundary. CLI
`run`, REPL/TUI prompts, ACP `session/prompt` and Crew's ACP bridge use
`configure_controller` and the same `AgentKernel`, `AgentSupervisor`, tool registry,
`LocalChildHost`, execution budgets and checkpoint path. Embedded core consumers
opt in with `with_coding_harness()`; mock/other consumers retain their existing
policy unless enabled.

## Policy and completion

The system policy directs execution until requested deliverables are completed,
failed with evidence, or suspended for a user-exclusive decision. Missing runners,
venvs, optional packages, pytest, uncloned repositories and no-match searches are
recoverable setup observations. Try installation, creation or fallback, record
local failures and continue independent work. Discover concrete items before
queueing; never substitute an environment-discovery checklist for deliverables.
Use selected dataset columns and isolated complete child inputs. An unavailable
official evaluator leaves `official_resolved=null` without preventing coding and
local validation. Save patches and reports before workspace cleanup.

This is advisory model policy, not an observed-progress gate or recovery lock.
In harness mode model-declared step subscopes are advisory: tools still enforce the
initial workspace boundary. A mistaken `workspace/workspace` declaration cannot
prevent inspecting the actual checkout or recovering within that boundary.
When a controller proposes text-only completion, known pending/running queue items
keep the same goal active. Both controller and child proposed finals use a streaming completion
review that checks original deliverables against tool evidence. It accepts completion,
requests concrete controller queue start/append, executes the next recovery tool
through the ordinary scheduler, continues recovery/reporting, or calls
structured `request_user_input`. The review uses the same retry/provider path and
counts toward step budgets. Synthetic completion checks do not count as executed
workspace tools; their model requests count as model rounds. Proposed controller text is emitted only after
acceptance. Reviews can still be wrong; they do not prove semantic code correctness.
Terminal queue summaries direct the controller to read authoritative current
receipts and create cross-item reports itself. Isolated reporting children need
complete report inputs, rather than assuming sibling output directories exist.
A persisted `[ax-completion-pending]` marker precedes each candidate final.
Recovery cannot treat that text as terminal until an accepted review acknowledgement
is durable; legacy histories without this marker retain their existing behavior.
Children do not create nested queues; the controller owns necessary user decisions.
A final child response with unresolved tool errors becomes a failed receipt even
after an accepted completion audit; reconnect recovers the candidate final rather
than treating the review acknowledgement as repair evidence.

## Concrete work activation

`task_source.work` accepts its schema-defined object and an explicitly JSON-encoded
object from model tool calls. Other shapes fail with a recoverable input diagnostic;
decoding does not alter projection, revision or ordering requirements.

`task_source` reads explicitly selected columns from parquet/JSON/CSV. Parquet
uses column projection before materializing rows. A `work` mapping supplies a title
column, complete per-item instruction, optional repository URL template/revision,
output root, and explicit ordering resource. Repository mappings must select an
explicit common revision or revision column; silently defaulting requested commits
to HEAD is rejected as recoverable input error. It returns the explicit typed envelope:

```json
{"ax_work_items":[{"title":"item","input":"complete standalone instructions",
 "workspace":{"mode":"git","repo_url":"https://example/repo.git","revision":"commit"},
 "output_dir":"absolute durable path"}]}
```

Any successful tool can return this envelope. The controller admits it before model
context projection, deduplicates identical title/input pairs, and selects children
when a host exists. Plain bullets, numbered rules and arbitrary table rows are
never parsed as tasks. Models may also use `task_queue start` directly for concrete
items. `append` preserves executed history, child receipts and task IDs while adding
newly discovered items and offsetting dependencies. It cannot erase prior work.

```text
Goal active → projected concrete inventory → start/append queue → ready children
→ task-local receipt → next independent ready item → all terminal → report/review
→ completed
```

Sequential mapping uses a shared write resource rather than success dependencies,
so a failed earlier item does not skip later items. Task-source relative output
roots resolve against its bound execution directory. JSON/CSV files are parsed
locally and return selected fields; parquet never loads answer columns unless they
are explicitly requested. This generic tool does not enforce a benchmark-specific
field allowlist: the caller's input contract determines projection. Evaluators
remain separate execution activities after patch freezing.

## Workspace lifecycle

`QueuedTask.workspace` is `WorkspaceSpec { mode: inherit|git|empty, repo_url?,
revision?, subdir? }`, defaulting to inherit for older queue snapshots. Optional
`output_dir` names durable output. Inherit uses the existing filtered controller
snapshot/dirty overlay. Empty creates an isolated directory. Git preparation occurs
before any child model request: validate spec, admit quota, obtain a URL-identified
bare object cache, resolve/fetch the revision to a commit, and checkout detached.
Sandbox-off mode uses lightweight worktrees; confined mode uses independent object
stores so siblings do not share writable Git metadata. Git hooks/fsmonitor are
disabled for lifecycle commands. Each child has its own session and memory scope.

`ChildRun.cwd` is the selected subdirectory; its optional `workspace_root` retains
lifecycle/sandbox/patch scope. Tool relative paths and shell working directories use
cwd, while confinement and final patch capture use repository root. Old snapshots
without workspace_root use cwd. Subdir traversal and escaping symlinks are rejected.
A preparation failure creates a unique failed task receipt, optionally writes the
requested output, and never prevents another independent item from starting.

## Environment and errors

Every main/child run receives compact `[ax-environment]` JSON. Executable path and
bounded version probes are lazy process-cached; cwd/root and sandbox information
are rebound per run. Example fields on Windows:

```json
{"os":"windows","shell":"Windows PowerShell 5.1 (powershell.exe)",
 "shell_contract":"Use PowerShell here-strings or python -c; no bash heredoc, && or ||. Use separate commands and check $LASTEXITCODE.",
 "cwd":"C:/runs/item/workspace/pkg","workspace_root":"C:/runs/item/workspace",
 "path_separator":"\\","executables":{"git":{"path":"C:/.../git.exe","version":"git version ..."}},
 "network_policy":"sandbox allows network; tool permissions still apply",
 "sandbox":"Off","write_boundary":"C:/runs/item/workspace"}
```

Executable absence is omission, version failure is null, and this cache is a startup
capability summary rather than a live package inventory. It may need a fresh process
after executable/PATH changes. Detection precedes turn-idle accounting.

Ordinary tool/setup errors remain recoverable/task-local. `task_queue block` in
harness mode requires `evidence_call_ids` referencing actual tool envelopes with a
typed `global_blocker`; prose alone cannot invent a global stop. Provider exhaustion,
persistence failure, cancellation and configured global budgets retain their actual
kernel failure/suspension paths. A necessary user decision uses structured
`request_user_input`, checkpoints WaitingForUser, and resumes the original goal.

Child context also includes `[ax-task-workspace]` with prepared WorkspaceSpec, cwd
and lifecycle root. Git children are told that checkout is already complete at the
requested revision and to edit that checkout rather than clone a nested duplicate.
This is execution context, not a runtime prohibition on recovery tools.

## Durable artifacts

Before successful/failed workspace cleanup, the host captures authoritative Git
status and binary/full-index patch (including ordinary new files), copies changed
files and declared artifacts, serializes diagnostics/validation/metrics and detailed
trace, then syncs atomic file replacements and saves the durable SQLite receipt.
State lives outside disposable workspace. Failure to preserve artifacts prevents
cleanup. GC also freezes unleased expired work and reads the saved output target.

Children write requested custom JSON/reports under `.ax-artifacts/`; the host exports
regular staging files to state and the validated output root. Runtime staging,
venvs and conventional build/cache directories are excluded from untracked patch
capture. Tracked changes are always retained. `child_result.json` is authoritative;
a custom `result.json` generated in the current staging directory is preserved in
output. Without a current custom result, the current receipt replaces an older
output result. Exported staging reports are listed as durable output artifacts. Measured metrics/trace files are
host-owned. Receipts include durable paths instead of deleted workspace paths.
Unknown provider token values are null in optional measured fields; legacy counters
remain compatible. Interrupted in-flight trace spans close with measured duration
and interrupted status, without inventing an error classification.

For non-Git `empty`/`inherit` workspaces, the host records content fingerprints
before execution and compares them before cleanup. This also captures shell edits
that do not emit file-edit events. Changed file contents survive cleanup; without a
Git base, `final.patch` is empty with an explicit diagnostic. Insertion/deletion
counts are nullable, including unavailable or binary Git counts. Missing model
usage in a retained round makes aggregate measured token fields null rather than
reporting a partial sum. Staging report writes do not count as the first code edit.
The controller advertises `child_result` before its first dispatch so newly created
receipts can be inspected in the same goal.

Configured child timeouts bound the total child model/tool loop, including
completion reviews; tool/model activity cannot renew that deadline. The controller
turn timeout remains idle-based so productive independent batches can outlive it.
A timed-out child saves its receipt/artifacts and independent tasks continue.

## Regression coverage

`test/harness/core.rs` covers typed dynamic 23-item admission, no bullet parsing,
pending completion rejection, streaming-only completion review, executed-queue
append, shared environment binding and local/global evidence classification.
`test/harness/task_source.rs` covers projected input isolation, 23 record mappings,
sequential failure continuation and relative output resolution.
`test/harness/workspaces.rs` prepares two local Git repositories at distinct exact
revisions, checks subdir cwd, executes real shell/edit tools after an earlier setup
failure, then verifies durable patch/receipt after cleanup and source isolation.
It also checks non-Git shell changes against the prepared content baseline and
retention beneath a runtime directory, with nullable diff counts.
`test/harness/child_deadline.rs` verifies that continuous model/tool activity cannot
refresh an explicit child deadline and the next independent item still completes.
Existing CLI prompt dispatch tests exercise CLI/TUI/ACP children (including
automatic typed 23-item dispatch on both CLI and ACP) and kernel tests
cover genuine request-user-input suspend/resume. These are harness tests, not a
benchmark-specific planner or correctness oracle.

### Current output evidence

Each frozen output has `artifact-manifest.json` identifying this child/task and its current exported artifacts. Existing unlisted custom files can be historical and must not supply current evaluation. `child_result.json` owns terminal status; custom `result.json` labels are descriptive. Cross-item reports use measured `metrics.json`, preserve unknown counts as null, and verify totals against terminal inventory. Terminal inventory remains injected after the queue is marked completed, including final completion review.

The current manifest includes host-generated receipt, trace, patch, metrics, validation and diagnostic files, even without child staging files. Setup failures publish the same receipt/manifest surface. Final review requires requested per-item reports for failed as well as completed items; unavailable measurements/evaluation remain null rather than excusing missing files.

Current child receipt status, measured metrics, diff statistics and grouped current output filenames are injected directly into controller terminal summarization and completion review. They do not require the model to first discover/call child_result. Host outputs are Artifact references as well as manifest entries. This remains evidence for advisory review, without observed-progress gates.
