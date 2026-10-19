# ADR 0029: Evidence-driven handoff after recoverable errors

Status: Accepted, 2026-10-18.

## Context

A release execution treated a client's missing login as an unavailable external
prerequisite without trying the documented existing authentication interface. It
also introduced malformed version lines through scripted replacement, attributed
the fault to existing files, and reported an unchanged project after changing its
version. Existing advisory recovery policy did not cause a review before those
user handoffs.

## Sources

- [OpenAI Codex prompting guide](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide): persistence within user scope, using dedicated editing tools, recovering from errors and verifying outcomes.
- [OpenAI Apply Patch](https://developers.openai.com/api/docs/guides/tools-apply-patch): structured file changes and explicit tool feedback.
- [Claude Code Stop hooks](https://code.claude.com/docs/en/hooks#stop): feedback may continue execution before stopping; bounded continuation avoids repeated-stop loops.
- [Anthropic effective harnesses](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents): explicit progress records and outcome verification reduce premature completion.

These published practices motivate AX's design below; they do not establish that
either product prevents every such failure or uses AX's exact implementation.

## Decision

Keep one execution loop and no task/prose classifier. Strengthen general policy
for completing authorized workflows, reusing supported existing authorization,
recovering own edits and reporting actual states. Correct shell guidance that
unconditionally claimed approval was required; permission decisions remain in
the existing runtime.

Only actual typed recoverable tool errors in the current host turn activate a
bounded handoff checkpoint. The first valid user question receives a normal
`review_required` result instead of suspending. An ordinary final gets one
same-model review continuation only if the latest tool-call round still has a
recoverable error. Successful follow-up operations/checks or recorded queue
outcomes keep direct completion rather than reviewing older failures again.
Each boundary can intervene once per host
turn, regardless of additional errors. Feedback references up to eight recent
failed call IDs, without duplicating command output or credentials. A model may
recover using tools, ask the evidenced user-only question again, or finalize
from sufficient evidence. No prescribed command or authentication workaround is
automatically executed.

Existing Goal verification owns its final boundary. A consumed finite step
budget suppresses the extra request. Typed permission denial/global blockers,
user pause/cancel and queued/steered work retain their existing handling. Error
receipts are read from raw current-turn messages, so compaction cannot remove
the trigger; old-turn failures cannot activate it. A pending candidate marker
prevents child recovery from mistaking an interrupted draft for completion.

## Tradeoffs

An errorful host turn can spend up to two additional execution-model requests;
successful direct turns spend none. A real blocker may be reviewed once before
the user sees its question. This is self-review, not independent verification,
and a model can still reason incorrectly after feedback. Streaming may expose a
candidate before correction; frontends must use TurnFinished as completion.
This design deliberately avoids a release-specific credential reader, automatic
access expansion, mandatory tests after every edit, or an unbounded stop guard.
