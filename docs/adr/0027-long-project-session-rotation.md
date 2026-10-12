# ADR 0027: Optional working context and Session rotation

Status: accepted

Long-running Goals need bounded model context and evidence-linked handoff without
introducing a second task, memory or execution store.

Use an optional LongProjectHost seam for a per-Goal Markdown projection. Keep
identity, criteria, cumulative consumption and execution state in the existing
Goal/Task Queue checkpoint. Default the feature off. Enable rotation only by an
explicit Long Project choice or a separately configured rotation policy.

Evaluate context pressure and measured Compaction gain at a safe model boundary.
Seed the ordinary successor Session before committing a redirect in the old JSONL
stream; restore only committed successors. Keep the kernel and permissions through
handoff. Preserve raw history and existing recovery barriers. A failed handoff
retains the predecessor and cannot admit completion or replay tools automatically.

Partition managed document blocks by participant and protect human edits with
content digests, compare-before-replace and an OS document lease. Archive old
context; never treat prose as authoritative task state. The existing fenced Crew
Workflow checkpoint can carry the document and digest for takeover; host ownership
and reconciliation remain unchanged.

See [Long Project](../long-project.md) for configuration, commands, recovery,
permissions and limitations. No new scheduler, recurring model invocation,
independent persistence service or reusable-memory store is added.
