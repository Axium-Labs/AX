# Self-Evolving Skills

AX observes ordinary sessions and learns reusable personal workflows without
adding a model request to every turn. The `evolution` crate extends the CLI
composition boundary; it has no dependency on runtime-core, tools, terminal UI
or permission policy. It can write only its private Skill territory and facts
through the existing Memory repository. It never executes generated code.

```text
Session → Experience → low-frequency analysis → candidate → trial → active
                                                   ↑         ↓       ↓
                                                   └── refine/merge ─┘
                                                          ↓
                                                      archived → deleted
```

## Observation and scheduling

The CLI wraps the existing `AgentEvent` callback. It records task text,
tool names, routed Skill names and observed Skill-file reads, ordered tool
steps, call outcomes, failures, possible retries, project identity and session
identity. Call IDs associate results with steps even when tools finish out of
order. Streaming text and private reasoning are not collected. Tasks and
details are bounded; steps and errors are limited to 64 per Experience.
Retries mean a repeated tool/detail after an observed failure, not an inferred
claim that an external side effect was replayed. `success` means the runtime
completed the turn; it is not proof that the user accepted the result. Errorful
turns remain in usage/error telemetry for the analyzer to interpret.
Cancellation drops the recorder and records an interrupted failure.

The normal request path hands data to a bounded, nonblocking worker queue.
File I/O, evidence screening and analysis run on an in-process worker thread;
the analysis uses the configured provider through the existing Tokio runtime.
No daemon or background service is installed. The queue has 64 slots; overload
reports a diagnostic and may drop a learning record, while authoritative
session history remains intact. Evolution failures do not fail the user turn.

Analysis is eligible after a batch accumulates or a session ends, and only
after the persisted cooldown. `/new`, session switching and project switching
signal the session boundary. `ax run` and idle TUI exit drain the worker; an
eligible final analysis can take up to its timeout. ACP rebuilds its composition
state for each prompt, so dropping that temporary state flushes observations
without falsely ending the session: ACP learning uses the accumulated batch
trigger. Abrupt process termination may lose queued observations; already
persisted evidence is available next time. A current TUI turn interrupted by
process exit is best effort, as with other pending work.

## Detection and classification

One bounded, tool-free model request reads recent Experiences and owned Skills.
It reuses the bundled `skills/skill-creator/SKILL.md` instructions. The analyzer
classifies durable preferences/facts as **Memory**, repeatable workflows as
**Skill**, and one-off/weak evidence as **Ignore**. Intent starts as the task
text; semantic intent and correction attribution happen during this analysis,
not through another per-turn request. REFINE corrections must quote actual
user text and are attached to the matching Experience records in the bounded runtime buffer. Raw observations
remain immutable in JSONL; correction counts and Skill evidence IDs remain in
lifecycle metadata.

The analyzer proposes JSON actions; it cannot choose paths or call tools.
AX creates and validates standard packages using the existing Skill crate's
`create_skill_directory` and `validate_skill_directory`. There is no alternate
Skill format or custom frontmatter. Analysis input is reduced to fit the
provider's declared context capacity, reserving its output capacity and the
creator/analysis instructions. Older raw observations remain on disk.

## Actions and lifecycle

The model proposes **CREATE / MERGE / REFINE / IGNORE / RETIRE** semantic
changes and **MEMORY** facts. **PROMOTE** explicitly requests candidate-to-trial
or trial-to-active. Runtime maintenance never promotes, archives or deletes
content because of a composite score or age.

Deterministic gates remain: at least `minimum_evidence` observations from
`independent_sessions` distinct nonempty sessions (defaults: 2 and 2), valid
project provenance, secret filtering, package validation, ownership/digest
protection, growth limits, and usage telemetry. Trial-to-active requires a
real successful use after the trial baseline. Analyzer `confidence` is
validated telemetry, not a lifecycle threshold. Failed observations can
support corrective lessons; success/failure semantics belong to the model.

REFINE preserves lifetime telemetry and resets changed active instructions
to trial. MERGE requires distinct owned sources and a smaller output, but
lexical similarity cannot veto the model's semantic merge. RETIRE archives
first; deletion requires a later explicit proposal and removes only the
original single-file package. Manual edits and added resources protect user
content. User-authored Skills and Memory are never automatically overwritten.

Candidates are stored outside discovery. Candidate-to-trial proposals require a
later analysis epoch and at least one evidence ID not in the creation/refinement
evidence. A valid proposal queues a content-digest-bound user review; it does not
promote automatically. The idle TUI displays the candidate instructions: `y`
accepts trial, `n` archives, and Esc leaves the candidate pending for a later TUI
session. Up/down scroll the instructions. Changed packages invalidate stale
reviews. ACP and `ax run` keep candidate state and never accept these reviews.
`ax evolution review` provides the same explicit decision outside the TUI.
Trial-to-active still requires the existing successful trial usage gates.
Trial and active Skills share a discoverable
directory and use model-directed skill selection, elastic context budgets, enable/disable
settings and tool permission checks. Archival immediately removes the package
from discovery. Other running sessions pick up changed package state on a later
Skill catalog refresh or restart. Evolved instructions are refreshed after
analysis and stripped from resumed snapshots so old archived instructions
cannot be restored.
Deleted entries retain bounded metadata tombstones and an append-only deletion
audit; raw session history and Experience
JSONL are never deleted by Evolution.

## Personalization switches

Evolution observation and worker preparation require both
`personalization.memory_enabled` and the independent
`personalization.evolution` switch. `personalization.tool_memory` controls
tool-driven Memory writes; it no longer controls Experience learning. New
installations default all three switches to `true`. When an existing
personalization configuration has no `evolution` field, its value inherits
that configuration's `tool_memory` value, preserving the previous learning
behavior on upgrade. An explicit `evolution` value takes precedence.

```bash
ax personalize --evolution false
ax personalize --tool-memory false --evolution true
ax personalize
```

`ax personalize` returns the resulting `evolution` boolean with the other
personalization fields in JSON. Turning Evolution off stops new Experience
recording and worker preparation; `--memory false` also prevents both. The
project Evolution configuration's `enabled` switch remains an additional gate
for recording and analysis. These switches do not remove learned packages or
disable inspection, review and archival. Existing live Skills can be disabled
through `/skills`.

## Evolution commands

`ax evolution` operates on the project containing the current directory,
including when invoked from a project subdirectory. It follows the normal
storage resolution and accepts the global `--data-dir` override; project UUIDs
still isolate projects sharing that directory. Each subcommand acquires the
same exclusive `<evolution-root>/writer.lock` as the learning worker before
opening the Engine or reading or changing Evolution data. A command waits for
the current writer and releases the lock when it finishes. The worker holds
this lock only while preparing an analysis or applying its result; it releases
the lock during the model request and response parsing, so management commands
can proceed while analysis is in flight. Inspection can
perform normal Engine recovery or legacy-ledger migration under that lock.
No provider request is needed.

All command results are JSON on stdout. `--help`, including subcommand help,
returns `{"help":"..."}`. A failed command or invalid Evolution arguments
return `{"error":"..."}` and a nonzero exit status.

```bash
ax evolution status
ax evolution skills
ax evolution show release-workflow
ax evolution review release-workflow --digest <digest> --accept
ax evolution review release-workflow --digest <digest> --reject
ax evolution archive release-workflow
```

- `status` returns `enabled`, `pending`, `last_analysis`,
  `consecutive_failures`, `backoff_secs` and `pending_reviews`. `enabled` is
  a boolean; the other fields are nonnegative integers. `enabled`
  combines both personalization gates with the project's Evolution `enabled`
  setting. `pending` counts Experiences awaiting analysis. `last_analysis` is
  the last persisted attempt time in Unix seconds (`0` means no attempt).
  `backoff_secs` is `0` when `consecutive_failures` is `0`. Otherwise it is
  the worker's current exponential retry interval, including the base cooldown;
  it is not the remaining wait. The ordinary cooldown still applies when
  there are no failures.
- `skills` returns an array of evolved Skill metadata with `name`, `state`,
  `use_count`, `success_count`, `failure_count`, `created_at`, `last_used_at`
  and `evidence_count`. Deleted tombstones are omitted. Timestamps use Unix
  seconds. Each entry includes `digest`: the queued review digest, or `null`
  when no review is pending (including non-candidate Skills). `name` and the
  lowercase `state` are strings; counters and timestamps are nonnegative
  integers; `digest` is a string or `null`.
- `show <name>` returns `name`, `state`, the complete file in `skill_md`, and
  `evidence` entries containing `id` and a `task` summary of at most 200 Unicode
  characters. Evidence is read from authoritative JSONL, including records
  outside the worker's recent buffer. `name`, `state` and `skill_md` are
  strings; `evidence` is an array of objects with string `id` and `task` fields.
- `review <name>` requires `--digest` and exactly one of `--accept` or
  `--reject`. It uses `Engine::review_candidate`: acceptance moves the queued
  candidate to trial; rejection archives it. A mismatched digest, changed
  package or absent pending review fails without accepting the candidate.
  Obtain the digest from `skills` and review the full instructions with `show`
  before deciding.
- `archive <name>` uses the existing owned-package lifecycle transition to
  archived state, removes the package from discovery and clears its pending
  review. Review and archive results contain `name` and the resulting `state`.
  Ownership, content-digest and lifecycle guards continue to apply; archival
  does not delete raw Experiences or session history.

## Ownership, persistence and configuration

All Evolution artifacts are project scoped beneath
`<data-dir>/evolution/<project-id>/`. Portable project UUIDs isolate evidence
and Skills even when multiple projects share a custom data directory:

```text
config.json           # optional overrides; missing fields use defaults
ledger.json           # versioned control state, byte cursors, Skill metadata
experiences.jsonl     # sole authoritative store of complete Experiences
decisions.jsonl       # action results and lifecycle transitions
writer.lock           # OS file lock; automatically released on process death
candidate/<name>/SKILL.md
live/<name>/SKILL.md   # trial and active only
archived/<name>/SKILL.md
```

RETIRE does not require creation evidence to remain in the recent window.
Archival checks package ownership/digest; deletion still requires another explicit
RETIRE in a later epoch. Analyzer packages include only Candidate, Trial and Active
bodies; archived metadata remains available for retirement proposals.
MEMORY and REFINE provenance quotes require at least eight trimmed Unicode
characters, or four CJK characters, and some letter/digit content; whitespace or
punctuation alone cannot establish user provenance.

`experiences.jsonl` is the source of truth. Recording validates scope and secret
screening, appends the complete unchanged `Experience`, and syncs it before
checkpointing control state. The version-2 ledger contains `version`, `epoch`,
`skills`, `pending`, `last_analysis`, `observed_cursor`, and `processed_cursor`;
it contains no Experience bodies or recent-evidence copies. `observed_cursor`
checkpoints usage telemetry with its counters; `processed_cursor` checkpoints
successful analysis consumption. Both are JSONL byte boundaries. Additive fields
`trial_requests`, `failed_batch_cursor`, `failed_batch_attempts` and
`consecutive_failures` persist reviews and recovery scheduling. Recording observes
new JSONL bytes and checkpoints the ledger once per new Experience; duplicates
without uncheckpointed bytes do not rewrite it. Legacy `Experience.intent` is
accepted when reading old data but is not serialized or sent to analysis.

The worker retains an Engine across commands. On its first use after restart,
it streams JSONL once to rebuild an in-memory ID/offset index and bounded recent
buffer. The index holds IDs and offsets, not Experience bodies, and grows with
the number of unique observations. Subsequent commands index only appended
bytes. Analysis seeks to the next pending bounded batch, retaining earlier
context where space permits. Provider context fitting cannot drop the earliest
pending observation and advance past it; excluded new observations remain
pending. A backlog larger than `max_experiences` is consumed across eligible
batches under the existing cooldown, rather than silently dropping old work.

Analysis tries each opening bracket, extracts the first parseable JSON array,
and tolerates bracketed prose, code fences and trailing prose. It audits/skips
individual invalid actions and truncates excess proposals to
`max_actions` with an audit. The analysis cursor normally advances after action
application or audited policy rejection and maintenance. Invalid model replies
persist a failure count identified by the pending batch's starting processed
cursor, so newly appended observations cannot reset its attempts. After three
invalid replies at that starting cursor, an explicit skip audit precedes advancing
the selected batch's end cursor; raw Experiences are retained. Timeouts and
provider/network errors only increase the persisted backoff count; they do not
count toward skipping or advance pending work. Audit/file/Memory/checkpoint
failures retain pending work and cannot authorize an unaudited skip. Attempt time
persists before requesting the provider. Consecutive failed analysis attempts
double the cooldown, capped at 24 hours, across restarts; a successful analysis
resets the backoff. The default request timeout is 120 seconds.
Preparation snapshots the selected batch's end cursor and its starting
`processed_cursor` while holding `writer.lock`, then persists `last_analysis`.
The model request and parsing run without the lock. Before applying a result,
the worker reacquires the lock and refreshes the Engine, preserving concurrent
observations and management changes. If `processed_cursor` has changed, the
worker discards the result and writes an `ANALYSIS_SUPERSEDED` audit with
`analysis superseded`; it does not apply actions, run maintenance, consume
evidence or alter failure/backoff counters for that obsolete attempt.
Replaying a JSONL-only append recovers telemetry once;
duplicate IDs are suppressed even outside the recent buffer and across restart.
As before, separate Skill files, Memory, audit and ledger writes do not form a
single transaction: a crash after an action but before cursor checkpointing may
retry analysis. Lifecycle ownership/digest gates and Memory optimistic writes
still protect their destinations; this is at-least-once analysis, not an
exactly-once cross-store transaction.
CREATE removes a just-written package if its ledger checkpoint fails. MERGE
preflights all source archive destinations, moves them as a batch and checkpoints
once; an ordinary move/checkpoint error restores sources and removes the new
package. These rollbacks do not claim crash-atomic multi-file transactions.

Legacy ledgers are migrated while the worker holds `writer.lock`. Existing JSONL
IDs are preserved without another append; legacy-only observations are appended.
JSONL wins if an old recent copy differs. The old pending suffix is mapped to a
byte cursor, including JSONL-only appends whose old checkpoint failed. Skill
metadata, usage counters, epoch and cooldown survive. Migration first checkpoints the original byte boundaries in a temporary
`experience_migration` control field, then atomically saves the version-2 ledger
without `experiences` or that temporary field. Interrupted migration retries by
ID using the original boundaries. A missing previously consumed legacy copy
appended after pending records can be conservatively analyzed again; existing
pending work is never skipped.
Derived per-Experience corrections previously present only in ledger copies are
not copied over authoritative raw observations; lifecycle correction counters
remain preserved. Do not run an older AX writer against a migrated store: it
expects the removed `experiences` field. Truncated streams, malformed records,
non-boundary cursors and torn final lines fail closed without rewriting raw data;
restore the stream from backup before resuming learning.

Metadata is separate from SKILL.md: `source`, `created_at`, `last_used_at`,
`use_count`, `success_count`, `failure_count`, `corrections`, `confidence`,
`state`, project ownership, content digest, evidence IDs and review baselines.
Only entries with `source=evolved`, matching project identity and unchanged
content digest can be automatically mutated. Built-in/user Skill roots have
precedence and are never mutation destinations. Known non-evolved names are
also excluded from proposals. Symlinks and Windows reparse points are rejected
in mutation territory. Manual edits stop automatic mutation of that package.

Memory learning initially stays Project scoped. Facts use the existing key,
value and credential validation and optimistic-write checks; user-authored
facts are never overwritten. Quoted user provenance must be present in the
selected evidence. Recognized credential-like Experience/Skill content is
excluded using the existing Memory screening; this is a conservative filter,
not a complete secret detector. Private learning data has the same local
privacy considerations as session history and is sent only to the selected
AX model provider for an eligible analysis. Evolution data is not yet included
in `.axpack` exports; normal learned Memory facts use existing export behavior.

Internally the crate is `config.rs` (limits), `types.rs` (experiences, ledger,
proposals), `engine.rs` (the single mutation boundary), `policy.rs` (the
lifecycle decision table the engine enforces), `storage.rs` (atomic writes,
digests, territory guards, credential screening) and `worker.rs` (the
off-startup analysis thread). `lib.rs` only re-exports.

Thresholds live centrally in `evolution::Config`. For example:

```json
{
  "enabled": true,
  "batch_size": 12,
  "cooldown_secs": 1800,
  "analysis_timeout_secs": 120,
  "max_experiences": 64,
  "max_skills": 24,
  "max_retained_skills": 48,
  "max_tombstones": 24,
  "max_skill_bytes": 12000,
  "max_actions": 4,
  "similarity": 0.86,
  "minimum_evidence": 2,
  "independent_sessions": 2,
  "create_score": 0.90,
  "trial_score": 0.65,
  "active_score": 0.78,
  "retire_score": 0.25,
  "half_life_secs": 2592000,
  "weights": [0.25, 0.20, 0.15, 0.10, 0.10, 0.20]
}
```

Legacy weights corresponded to recurrence, success, recent usage, correction
stability, project relevance and confidence; they no longer participate in
decisions. Project relevance is conservative ownership
scoping: this first implementation does not generalize across projects. Batch
size and cooldown are scheduling controls, not lifecycle rules. The live cap
counts candidates, trial and active Skills. A separate retained-package cap
bounds their combined size with archives; when full, only refinement/compression
and retirement can free room. Archived packages remain reviewable until utility
supports deletion. Tombstones are bounded separately and full deletion metadata
is retained in the audit. The size budget applies to all generated and
refined bodies, while merges must additionally shrink total source content.
Invalid configuration fails closed and reports a diagnostic. `enabled=false`
disables recording and analysis; existing live Skills can still be disabled
using `/skills`. Configuration is reread by the worker for each observation.

The unit/integration suite covers ownership guards, standard format, routing
states, trial promotion, correction/retrial counters, merge compression,
archive/delete ordering, memory provenance, cooldown, disabled operation,
credential screening, context fitting, model-call frequency and failed analysis.
Persistence coverage also includes legacy migration, JSONL-only append recovery,
incremental byte reads, bounded backlog batches, window-independent deduplication,
failed cursor/audit checkpoints, restart consumption and torn/truncated streams.

Legacy `similarity`, `create_score`, `trial_score`, `active_score`,
`retire_score`, `half_life_secs`, and `weights` remain accepted for existing
config files. They do not decide semantic creation, promotion or retirement.
Existing ownership ledgers and usage counters are retained; no database
migration or vector database is required.
The analyzer input omits Config entirely. Legacy scoring config fields are
deserialization compatibility only, not model input or lifecycle policy. See
[ADR 0034](adr/0034-reviewed-evolution-and-analysis-recovery.md).
