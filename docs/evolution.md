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

The CLI wraps the existing `AgentEvent` callback. It records task/intent,
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
user text and are attached to the matching Experience records in the ledger.

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

Candidates are stored outside discovery. A later analysis can move a credible,
still relevant candidate to trial. Trial and active Skills share a discoverable
directory and use model-directed skill selection, elastic context budgets, enable/disable
settings and tool permission checks. Archival immediately removes the package
from discovery. Evolved instructions are refreshed after analysis and stripped
from resumed snapshots so old archived instructions cannot be restored.
Deleted entries retain bounded metadata tombstones and an append-only deletion
audit; raw session history and Experience
JSONL are never deleted by Evolution.

## Ownership, persistence and configuration

All Evolution artifacts are project scoped beneath
`<data-dir>/evolution/<project-id>/`. Portable project UUIDs isolate evidence
and Skills even when multiple projects share a custom data directory:

```text
config.json           # optional overrides; missing fields use defaults
ledger.json           # bounded recent evidence, scheduling state, Skill metadata
experiences.jsonl     # append-only observations
decisions.jsonl       # action results and lifecycle transitions
writer.lock           # OS file lock; automatically released on process death
candidate/<name>/SKILL.md
live/<name>/SKILL.md   # trial and active only
archived/<name>/SKILL.md
```

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
  "analysis_timeout_secs": 30,
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

Weights correspond to recurrence, success, recent usage, correction stability,
project relevance and confidence. Project relevance is conservative ownership
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

Legacy `similarity`, `create_score`, `trial_score`, `active_score`,
`retire_score`, `half_life_secs`, and `weights` remain accepted for existing
config files. They do not decide semantic creation, promotion or retirement.
Existing ownership ledgers and usage counters are retained; no database
migration or vector database is required.
