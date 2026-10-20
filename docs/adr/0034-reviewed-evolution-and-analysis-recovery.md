# 0034. Reviewed Evolution promotion and bounded analysis recovery

Status: accepted; supplements ADR 0006 and ADR 0017.

## Context

Creation evidence aged out of the runtime window, preventing retirement. A single
malformed analyzer action could poison a pending batch forever. Immediate
candidate promotion could expose newly generated instructions without later
evidence or user review. Ledger failures could leave untracked Skill directories.

## Decision

Retirement uses ownership/digest and a later explicit RETIRE for deletion, without
requiring original evidence. Candidate promotion requires a later epoch and new
evidence, then queues a content-digest-bound TUI review. Accept enters trial;
reject archives; ACP/run cannot accept. Trial-to-active usage gates remain.

Try each opening bracket for the first parseable JSON array; audit invalid actions
and max_actions truncation. Persist invalid-reply attempts by the pending batch's
starting processed cursor, independent of new appended observations. After three
invalid replies, audit the skip before advancing only the selected batch; timeouts
and provider/network errors affect backoff only. Persist exponential cooldown, capped
at 24 hours; successful analysis resets it. Keep the 120-second timeout in Config.
Persistence/audit errors cannot advance failed work. Raw JSONL remains immutable.

Record usage/cursors once per Experience checkpoint. Do not serialize duplicate
intent or expose legacy scoring config to the analyzer. Keep legacy read support.
Require substantive quoted user provenance. Exclude archived bodies from packages.
Roll back a newly created package on ledger failure; preflight MERGE source
archives and roll back ordinary move/checkpoint failures as one operation.

## Consequences

Learning remains best effort and outside the kernel. Three invalid model replies
may skip learning from one batch, with durable audit and raw data for inspection.
Candidates can remain pending indefinitely without a TUI review. Files, ledger,
Memory and audits still do not form a crash-atomic transaction. Existing legacy
configuration fields remain readable but have no semantic decision authority.
