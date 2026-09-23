# 0001. Scoped memory with portable project identity

Status: accepted

## Context

AX needed durable memory that (a) distinguishes facts meant for one
conversation, one project, or everything, and (b) keeps project-scoped data
attached to the *project* rather than to an absolute filesystem path — so a
project can be moved, renamed or checked out elsewhere without losing its
memories.

Earlier records were owned by the project's absolute path, which broke on
move/rename and made shared custom databases ambiguous. Fact keys derived from
sentence hashes made natural-language statements hard to update reliably.

## Decision

- Facts live in three scopes: **Global** (AX home), **Project** (a portable
  UUID), and **Session** (one session ID).
- Project ownership is a UUID persisted in `.ax/project.json`, created on
  first use, independent of `--data-dir` and the workspace path.
- Same-key scope precedence is Session → Project → Global.
- Fact writes require a verbatim excerpt of the current user request
  (provenance); updates and deletes require the exact previous value
  (optimistic concurrency).
- Legacy path-owned records and sentence-hash keys migrate transactionally;
  migration markers prevent resurrecting deleted facts.

## Consequences

- Moving or copying `.ax` with the project carries its identity; a fresh clone
  without `.ax` starts a new identity (deliberate).
- Shared custom databases never infer ownership from an unmatched path; legacy
  unowned project facts there need explicit migration.
- Durable writes are safer but require explicit user intent: temporary or
  ambiguous requests stay Session-scoped by default.
- Retrieval is bounded by a shared token budget so memory never crowds out
  conversation — see [memory.md](../memory.md).
