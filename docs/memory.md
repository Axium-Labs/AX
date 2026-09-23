# Memory behavior

AX stores raw session history, effective context snapshots, and scoped facts in SQLite. Context compaction only changes effective context. It never promotes a summary to global or project memory and never deletes raw messages.

## Scope and writes

- Global facts belong to the current AX home and can be retrieved in any project.
- Project facts belong to a portable UUID in the project's `.ax/project-id`.
- Session facts belong to one session ID and are not inherited by a new session.

Structured declarations are supported without an extra model call:

```text
remember response.detail=brief
project: remember build.command=cargo test
global: remember response.language=English
```

The first example is session-local. Natural language requests are interpreted by the main model through the `memory` tool. That tool lists, sets, or deletes facts. It instructs the model to use session scope for temporary or ambiguous requests and durable scopes only when the user asks for reuse. It requires a verbatim excerpt of the current user request for a mutation. The excerpt proves provenance; deciding its meaning and authorization is the model's responsibility.

The model lists existing facts and reuses their keys when changing the same fact. Updates and deletions require the exact previous value; stale writes fail without replacing newer data. Natural language statements are no longer assigned sentence-hash keys. Older hash-keyed records remain readable and can be updated or deleted using their existing keys.

All fact-writing APIs validate keys and values for empty/oversized data and known credential patterns. Retrieval also excludes legacy records that fail this validation. This is a conservative filter, not a guarantee that every possible secret format is recognized. Raw conversation history is retained separately and is not redacted by this filter.

## Portable project identity

Project-root discovery still locates the repository or project directory. The directory's absolute path is no longer the owner of new facts. The first use creates a UUID in `.ax/project-id`, independent of `--data-dir`. An existing identity also identifies a project without Git or package markers when starting in a subdirectory.

Move `.ax` with the project to retain both the identity and the default database. Copying `.ax/project-id` deliberately copies the identity; a fresh clone without `.ax` gets a new identity. Deleting the ID loses this association. With a custom data directory, keep that database accessible as well.

Legacy path-owned records migrate transactionally to the UUID. Matching the current old path is allowed in any store. A project-local default database can also adopt a single old path after a move. Shared custom databases never infer ownership from an unmatched path. Existing UUID-owned values win conflicts; original records remain available in the database for export, and migration markers prevent deleted facts from being resurrected. Legacy unowned project facts in a shared custom database require explicit migration.

## Retrieval and context budget

Same-key scope precedence is Session, Project, then Global. Within the selected facts, lexical relevance determines ordering and update time breaks ties. Only explicitly marked `always_include` Global preferences may be selected without query relevance. A `preference.*` key alone does not grant that behavior.

Up to 32 relevant candidates are considered for a compact injected message. Its total size, including the wrapper, must fit `ContextBudget::memory_budget_tokens()` (at most 512 estimated tokens, reduced for small contexts). Oversized candidates are skipped so smaller relevant facts can still fit. Sources and timestamps are available in the manager and tool listing instead of repeated in every injected context. Final request selection still enforces the overall context budget.

`[memory.retrieve]` logs candidate, matched, injected, dropped-for-budget, token, and budget counts without logging fact contents. Retrieval runs at the start of each user turn. Tool updates are returned to the model immediately; the injected memory block is refreshed on the following user turn.

## Persistence and resume

The CLI checkpoints each complete user message, assistant response, and tool result before execution advances. Persistence failure stops the turn. Streaming text that has not yet formed a complete response is not checkpointed. Compression snapshots are saved at the end of the turn, after raw messages have been saved, and are anchored to the latest persisted message ID.

Resume queries messages after the snapshot watermark in backward pages of 128. It stops once enough recent history has been read for the token budget and a user-turn boundary is available. Persistent agent state is restored separately for legacy summaries. One exceptionally large latest turn can still require multiple pages. The initial UI transcript also loads only the latest page; full-history inspection remains a separate action under `/memory`.

An assistant tool call saved just before a crash may lack its result. Resume appends an explicit interrupted-result marker. It does not replay the tool: external side effects may already have happened. The model is told to inspect current state before retrying.

## User controls

`/memory` opens Global, Project, and Session views. Select a fact to inspect its value preview, source, update age, and inclusion setting. Edit opens a text editor; Alt+Enter saves and Esc cancels. Delete removes that fact in its current scope. An edit rejected because of validation or a concurrent update is reported instead of overwriting the current record. Close and reopen the record to load a newer baseline.

Session actions also expose original history, the current compression summary, manual compaction, and clearing Session facts. Deleting a session removes its Session facts, messages, and summary through the existing session deletion path.
