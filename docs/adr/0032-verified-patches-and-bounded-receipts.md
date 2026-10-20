# 0032. Verified patches and bounded receipts

Status: accepted

## Context

Line coordinates alone cannot detect stale replacements. Patch confirmations
did not show the resulting local context. Identical discovery responses repeated
full bodies in model context, and retained receipt storage grew without a byte cap.
Raw history must remain authoritative and operational limits must be explicit
or derive from ContextBudget.

## Decision

Require `expected_lines` whenever `delete_count > 0`, validate every hunk before
writing, and reject missing expectations as invalid input. Successful receipts
include verbatim changed regions with final line numbers and up to two surrounding
lines on each side. Each hunk follows its own original region's last line ending
(insertions follow the preceding line). An EOF insertion terminates an unterminated
preceding line before inserting text; other unedited bytes stay intact.

Same-round identical discovery responses contain a short `reused_from` reference
to the first call instead of another body or diagnostic array. Status and
truncated state survive; typed same-round result dependencies follow the reference
to the original receipt. The first response and raw history retain the evidence.

All retained-output cache writes, including restored legacy entries, use one FIFO
4 MiB `MAX_RETAINED_OUTPUT_BYTES` cap, estimated from ID and raw-output lengths.
This independent cache limit avoids coupling receipt retention to file-read policy,
and raw-byte estimates avoid allocating a serialized copy on every cache operation.
Oldest entries are evicted; individually oversized receipts are not cached.
Unknown or evicted IDs explicitly request a file reread or original-tool rerun.
No unbounded eviction tombstones or startup work are added.

## Consequences

Older callers must supply expectations for replacements/deletions. Patch output
supports immediate inspection but does not certify syntax or builds. Models see
one discovery body, while dependent typed calls retain the original values.
Receipt availability is finite even after restore; persistent history remains
lossless and is independent of this convenience cache. The named retention limit
supplies a deterministic payload bound without a core-to-tool dependency.
