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
and next `start_line` when clipped. When the very first line is itself larger
than the allowance, its beginning is retained up to a UTF-8 character boundary,
`line_truncated` is set, and the next `start_line` is the following line so a
reread always advances; the view points the model at `search` for content deeper
inside that line. Reuse, request preparation and Session rotation preserve the
typed envelope. Raw history remains authoritative.

Restoring a Session rebuilds the receipt cache from saved tool messages, which
may already hold a compacted projection. A truncated envelope therefore never
replaces a complete receipt for the same call, and `tool_output` refuses a range
that a truncated receipt no longer retains instead of returning an empty body.

Typed data consumers use `ToolResult::source_text()` to remove the exact declared
sequence of coordinate prefixes on a separate copy. This preserves JSON pointers,
work inventories and execution observations while leaving saved and model-visible
source bodies unchanged.

## Consequences

File context remains suitable for patching after removing coordinate prefixes.
Command output keeps its existing cleanup and old snapshots retain ordinary text
semantics. Tools producing mixed output can choose the variant for each call
without a registry-wide tool-name exception. A source line larger than the whole
allowance still advances: only its beginning is shown, `line_truncated` marks the
partial line, and `search` is the way to locate content inside it, so a reread
never repeats an empty line. Rotation shrinks retained results by halving them
per pass, which bounds the work logarithmically instead of re-serializing the
body once per removed line. Semantic compression can still summarize older
turns; the full original receipt remains in raw history.
