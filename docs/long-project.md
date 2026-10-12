# Long Project and automatic Session rotation

Long Project is an optional Goal working-context projection. It is off by default.
Enabling it explicitly enables automatic rotation by default. Neither feature
creates another agent, scheduler, memory service or execution database. Goal,
Task Queue, Harness/Checkpoint, the existing Session JSONL stream and SQLite
remain the execution and recovery authorities.

## Enable and manage

Create an explicit Goal with the existing native command (TUI, `ax run`, or ACP):

```text
/goal create {"target":"maintain the project","acceptance":["changes are verified against measured results"],"long_project":{"enabled":true}}
/goal status
/goal pause
/goal project {"enabled":true,"rotation":{"automatic":false}}
/goal resume
/goal rotate
/goal project show
/goal project export project-state-copy.md
/goal project import reviewed-project-state.md
/goal project history receipt-reference
```

Configuration changes to a saved Goal require a paused, blocked or created Goal.
They do not execute work. Resume explicitly afterwards. Manual rotation resumes
an unfinished Goal and requests a handoff at its next safe model boundary; it
works with automatic rotation disabled. Pending user input must be answered
first. Terminal Goals cannot be restarted by rotation.

`project show`, import/export and archive search are provider-free native
commands. The document is ordinary Markdown and can also be edited with an
editor. Export requires a new destination; import preserves the previous document
in the archive and does not replace Goal criteria, budgets, permissions or Task
state. Disable with `{"enabled":false}`; documents and history remain in place.
Re-enable to continue using them. AX does not enable this feature by classifying
a user's ordinary prompt.

## Configuration scopes

The existing global `config.json` **goal** template and project `.ax/goal.json`
accept `long_project`. Its nested fields merge global → project → per-Goal JSON;
an explicit `null` removes the inherited Long Project configuration. Other Goal
template fields retain their existing whole-project-template behavior. Saved
Goals freeze their resolved configuration. `ax goal-settings --scope global|project`
uses the same validation and persistence path; editing templates never starts
execution.

```json
{
  "long_project": {
    "enabled": false,
    "directory": null,
    "template": null,
    "sections": ["目标与约束", "当前状态", "已完成工作", "待办任务", "关键决策", "问题与风险", "运行环境", "历史索引"],
    "save_content": ["completed_work", "todo_tasks", "tool_references", "decisions", "issues", "environment", "history", "user_guidance"],
    "auto_update": true,
    "update_on": ["state", "stage", "decision", "handoff"],
    "max_document_bytes": 65536,
    "evidence_entries": 24,
    "stale_after_seconds": 86400,
    "rotation": {
      "automatic": true,
      "strategy": "after_compaction",
      "context_percent": 85,
      "minimum_compaction_gain_percent": 20,
      "retain_recent_tools": true,
      "retain_user_guidance": true,
      "retain_compaction_summary": true
    }
  }
}
```

`template` is literal Markdown. `{{goal_id}}`, `{{target}}`, `{{acceptance}}`
are safe substituted fields; other custom fields/text stay user-owned. With no
template the eight configured headings form the built-in template. Users can
rename/add/remove nonrequired sections and use the `long_project` tool for
corresponding evidence-linked notes. A required runtime checkpoint reference
remains independent of the template so removing a heading cannot delete
execution identity or turn prose into authority.

`save_content` selects optional projected fields using the names above, including
an empty list for a minimal document. Goal identity, target, acceptance, current
state and checkpoint source remain mandatory. Custom fields are literal template
text or evidence-linked section notes. Removing a projected field does not delete
older archives or user-authored content, or change authoritative Task state.

`directory` is an optional explicitly selected base directory, relative to the
project or absolute for a shared filesystem. The default is `.ax/long-projects`.
Each Goal uses `<base>/<sha256(goal_id)>/PROJECT_STATE.md`, preventing identifier
path traversal and collisions between different Goals. The byte limit is a
storage bound (1 KiB–1 MiB), not a context-window policy. Model excerpts always
use ContextBudget. `evidence_entries` bounds current task/call references; full
records stay in existing checkpoints and historical Sessions. Whole-document
updates that would exceed the limit fail without replacing the old document.

For ordinary tasks, rotation is separately opt-in: global `session_rotation`
uses the same rotation object and defaults to `null`. An explicit Goal can use
`GoalConfig.session_rotation` without enabling Long Project. Enabled Long Project
uses its own rotation configuration first; otherwise the Goal override wins over
the global policy. This does not enable Long Project or create its document.

## Evidence and incremental document maintenance

The host projects actual queue task status, dependency indices, Goal state,
acceptance, successful structured call IDs, activity, decisions and source
Session/checkpoint references. Task order is the queue's existing order; no
parallel priority/task system is introduced. Completed task state and successful
calls mean exactly what their saved receipts establish, not semantic acceptance
of assistant claims. Deliverables, commits, environment facts and artifacts can
be referenced in evidence-linked notes, without copying entire tool outputs.

The `long_project` tool is registered only for an enabled Goal. It runs through
the existing Scheduler, resource leases, capability permissions and runtime-owned
filesystem boundary. It is not inherited by child runs. Its actions are:

- `record`: section, concise text and `sources` (successful current Goal call IDs
  or completed `task:N`). Unknown/failed sources and sensitive content are rejected.
  Notes explicitly remain agent annotations requiring source verification, never
  completion proof. `expected_revision` may be supplied; otherwise the host's
  last observed document revision is the baseline. A conflict requires read/merge.
- `read`: document, revision and freshness metadata.
- `history` with `query`: retrieve archived context on demand.
- `history` with `session`, `call_id`, optional `before_message_id`: read existing
  raw Session tool receipts without executing the operation again. The Session
  must have the same Goal identity and the call must be successful in the current
  Goal checkpoint. Existing MemoryStore pagination bounds each retrieval; a
  continuation cursor is returned when another page is needed.

Automatic updates compare actual state signatures and update managed blocks
only at configured state/stage/decision boundaries or handoff. Assistant prose
alone does not trigger a rewrite. `auto_update:false` disables ordinary projection
refreshes, while first initialization and necessary handoff references remain.
Old versions are archived under `archive/<revision>.md`. One current note per
section/participant replaces that participant's previous intact note; older
versions remain retrievable. No model request is made to generate the template,
refresh the projection, create an archive or transfer context.

Managed blocks carry content digests. An edited block is retained byte-for-byte
and new runtime facts go into a separate current block; free user text is never
unconditionally regenerated. Every writer re-reads under the document lease and
compares the expected bytes before replacement. Runtime/note blocks are scoped
to the participant's first Session identity, retained across that participant's
rotation chain. Different AX participants merge their own blocks instead of
replacing each other's state. Size/lease/revision/permission errors stop advancement
with an explicit resumable error.

Automatic prose handling omits credential-like lines, PEM material and opaque
credential-like values. Raw tool output is never copied automatically into the
working document. Imports, automatic archives and distributed document snapshots
are filtered as well. These filters are conservative; this feature is not a
credential vault. User-authored original files remain user-owned.

Timestamped observations have a configurable expiry check. Expired and
untimestamped/manual context require checking current source receipts and actual
environment before use; AX does not pretend to have revalidated a remote system
by changing a timestamp. Necessary revalidation uses ordinary tools on demand.
Decision annotations are placed ahead of a potentially long custom template in
the bounded model excerpt. Omitted content has an explicit retrieval hint.

## Rotation and recovery

The loop evaluates the policy after its existing Compaction pass, before the
next model request. Pressure uses estimated effective-context tokens plus real
projected request growth, divided by `ContextBudget.usable()` (reply and tool
schema reserves excluded). It never uses message count as its sole trigger.

- `pressure`: rotate when configured pressure is reached.
- `after_compaction` (default): rotate under pressure when compaction provides
  insufficient measured gain, or pressure remains without an applicable pass.
- `stage_boundary`: under pressure, rotate when additional queue work has
  completed since the previous handoff. The same completed stage does not
  trigger repeated handoffs. Pending tasks remain pending; completed work
  is not resubmitted.

Automatic handoffs must also free the configured minimum share of context.
This avoids repeated empty rotations when persistent instructions dominate the
window. Oversize required handoff context pauses safely instead of discarding
instructions to fit. A manual request can rotate without the automatic gain
requirement. Tools, approvals, retries, user questions, running children and
in-flight validation/reservations defer handoff; no running work is interrupted.

The save/activate order is:

1. Flush old raw messages, current Goal/queue checkpoint and the working document.
2. Build bounded context: runtime/permission instructions, Goal definition,
   working notes, current queue, necessary Memory, user guidance, explicitly
   unverified historical summary, and the latest completed tool round.
3. Check the complete context against ContextBudget. Large tool outputs are
   projected while retaining call/result pairs; raw latest-round results use the
   existing optional agent-state checkpoint for the `tool_output` reader.
4. Seed a new ordinary Session and its effective-context watermark, queue,
   ExecutionState and child receipts. Save a predecessor reference.
5. Append the successor commit to the old authoritative JSONL stream. Only then
   activate the new Session. Retain the same kernel, Goal ID, cumulative usage,
   acceptance, execution limits, permission ceiling and child host.
6. Clear the old per-turn raw buffer and old raw-result cache entries, retaining
   the latest round. Old Session histories and archived context remain on disk.

Session leases protect both sides throughout a turn. A crash before the successor
commit leaves the predecessor authoritative; an incompletely seeded successor
cannot open/execute. A crash after commit follows the chain to the new Session.
Old Session references resolve to the latest committed successor for continued
Goal management; historical JSONL streams remain available for viewing/retrieval.
Opening a saved Session does not automatically execute work. Existing Goal
restore reconciles queue state, child receipts, in-flight usage and unsafe verifier
barriers before explicit resume. All uncertainty remains visible; rotation never
admits completion and never resets consumption.

CLI, TUI and ACP receive a `SessionRotated` event. ACP supplies
`_meta.axSessionRotation` with Goal ID and old/new Session IDs; the original ACP
conversation can continue addressing its existing Session handle through the
committed chain. No terminal output bypasses the TUI event renderer.

## Distributed use and limits

A shared filesystem must support the OS file leases and atomic rename used by
all AX writers. Existing OS permissions/ACLs decide access; errors fail closed,
symlink paths are rejected and no access grant is inferred from a template. Human
editors should finish a save before AX's next update; unrelated editors do not
participate in AX's advisory lease protocol.

The existing AXCrew Workflow checkpoint also carries an optional filtered
`long_project_document` and its digest, alongside the same queue and bounded
messages. Existing Workflow revision CAS, owner incarnation/generation and gateway
permissions still fence writes. Before a Long Project boundary the runtime checks
that host checkpoint seam. Takeover validates the document digest and merges it
without overwriting local user context. No AXCrew schema, scheduler, new memory
store or global budget ledger is introduced.

Full local Session databases and child workspaces are not migrated by this
adapter. Existing takeover rules still require host reconciliation of ordinary
external effects and local child work before execution on a different machine.
Distributed task ownership remains AXCrew's responsibility; sharing Markdown does
not grant two controllers authority over one distributed task. Archive transfer
can use the existing Artifact mechanism or a shared directory when history beyond
the current working document is needed. Long Project maintains working context;
Memory retains reusable cross-task knowledge; Task/Checkpoint supplies reliable
execution and recovery.

There is no timer, background wake-up or guarantee of execution while AX is
closed. Long-lived Goals continue through existing explicit execution/resume and
host scheduling. This design supports many bounded Sessions across a long
lifecycle; it does not require one ever-growing context or claim that a
months/years-long live run has already been observed.


AXCrew's collaboration workspace presents Long Project inline within an enabled
Goal. Live commands use native project show/import/config/history and rotation;
mutation success includes the existing fenced Workflow checkpoint, keeping the
editable document projection and saved Goal policy synchronized for takeover.
Reported successor Session IDs extend the same attempt history; editing prose
never resets cumulative usage or changes authoritative execution/permissions.
