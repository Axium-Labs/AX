# 0031. Tool-declared verbatim output

Status: accepted

## Context

Tool result cleanup was designed for command output: it pretty-printed JSON,
removed warning lines and extracted diagnostic-looking text. Applying it to file
reads changed source whitespace, omitted valid lines and presented ordinary code
as errors. Character clipping also left file reads without usable continuation
coordinates. The runtime must remain tool-agnostic, preserve old snapshots and
derive context limits from `ContextBudget`.

## Decision

Tools declare source semantics per successful execution through the existing
typed output boundary: `ToolOutput::Verbatim { text, total_lines, start_line }`.
`Tool::execute_output` defaults to ordinary `Text`, so existing tools keep their
behavior. The scheduler stores the declaration in additive, serde-defaulted
`ToolResult` fields; it never branches on tool names or infers source semantics
from text. File reads use this variant for full files and ranges, with the same
`N: ` coordinate prefixes and unchanged source line terminators.

Verbatim envelopes skip diagnostic extraction, JSON formatting and warning
filtering. Model projection and deterministic cleanup retain complete leading
lines within the allowance supplied by `ContextBudget`, preserving total lines
and the original starting coordinate. Model views expose the last displayed line
and next `start_line` when clipped. Reuse, request preparation and Session
rotation preserve the typed envelope. Raw history remains authoritative.

Typed data consumers use `ToolResult::source_text()` to remove the exact declared
sequence of coordinate prefixes on a separate copy. This preserves JSON pointers,
work inventories and execution observations while leaving saved and model-visible
source bodies unchanged.

## Consequences

File context remains suitable for patching after removing coordinate prefixes.
Command output keeps its existing cleanup and old snapshots retain ordinary text
semantics. Tools producing mixed output can choose the variant for each call
without a registry-wide tool-name exception. A source line larger than the whole
allowance cannot be partially displayed: its projection is empty and points to
the same undisplayed line. Semantic compression can still summarize older turns;
the full original receipt remains in raw history.
