# Memory

AX stores raw session history, effective-context snapshots and scoped facts in
SQLite. Context compaction only changes effective context — it never promotes
a summary to global or project memory and never deletes raw messages. The
on-disk layout and schema are documented in [storage.md](storage.md).
Portable export and import are documented in [backup.md](backup.md).

## Scopes and writes

Facts live in three scopes:

- **Global** — belong to the current installation AX home (`<install-dir>/.ax`) and can be retrieved in
  any project.
- **Project** — belong to a UUID in the installation-owned project store;
  they remain isolated from other projects and survive workspace deletion.
- **Session** — belong to one session ID and are not inherited by a new
  session.

Structured declarations are supported without an extra model call:

```text
remember response.detail=brief
project: remember build.command=cargo test
global: remember response.language=English
```

The first example is session-local. Natural-language requests are interpreted
by the main model through the session-bound `memory` tool. That tool lists,
sets or deletes facts; it instructs the model to use Session scope for
temporary or ambiguous requests and durable scopes only when the user asks for
reuse. A mutation requires a verbatim excerpt of the current user request —
the excerpt proves provenance; deciding its meaning and authorization is the
model's responsibility.

The model lists existing facts and reuses their keys when changing the same
fact. Updates and deletions require the exact previous value; stale writes
fail without replacing newer data. Natural-language statements are no longer
assigned sentence-hash keys; older hash-keyed records remain readable and can
be updated or deleted with their existing keys.

All fact-writing APIs validate keys and values for empty/oversized data and
known credential patterns. Retrieval also excludes legacy records that fail
this validation — a conservative filter, not a guarantee that every secret
format is recognized. Raw conversation history is retained separately and is
not redacted by this filter.

## Project identity

Project-root discovery locates the repository or project directory. UUID ownership
is independent of `--data-dir`; metadata lives in the installation-owned project
store. Legacy workspace IDs remain readable and are retained during migration.
Known installation identities also identify bare project roots from subdirectories.
Deleting workspace contents preserves both stored identity and history. A new path
without a legacy ID has a new identity; use export/import to transfer its data.

Legacy path-owned records migrate transactionally to the UUID. Matching the
current old path is allowed in any store; a project-local default database can
adopt a single old path after a move; shared custom databases never infer
ownership from an unmatched path. Existing UUID-owned values win conflicts,
original records remain available for export, migration markers prevent
deleted facts from being resurrected, and legacy unowned project facts in a
shared custom database require explicit migration.

## Retrieval and context budget

Retrieval has four stages, all local with no embeddings, vector database, or
additional model call:

1. Filter ownership to the current Session, current Project UUID, and Global.
   Other projects and sessions never enter recall. Superseded/expired records
   stay available through existing management APIs but are excluded here.
   Active same-key precedence remains Session > Project > Global.
2. Recall at most 32 candidates using the existing language-independent
   lexical features (NFKC, case fold, words and Unicode n-grams), exact key
   matches, tags and paths. Only explicitly pinned Global records bypass the
   relevance floor; a preference key alone does not pin a record.
3. Lightly rerank recalled candidates using relevance plus small bounded usage,
   confidence and type-aware freshness bonuses. Scope and source never contribute
   to score. Usage is logarithmic and saturates at 32 uses; confidence is an
   explicit integer from 0 to 100 (legacy default: 50).
4. Inject at most six index summaries, each a prefix of up to 160 Unicode
   characters. Full values are fetched by the session-bound `memory` tool with
   `action="read"`, `scope`, and `key`; `action="index"` browses summaries.
   Existing `list`, `set`, `delete` and scoped storage APIs remain supported.

Memory types define wall-clock decay since the content's `updated_at`:

| Type | Freshness half-life |
|---|---|
| `preference` | No decay; explicitly replace or expire it |
| `fact`, `decision`, `reference` | 180 days |
| `experience` | 30 days |
| `task` | 3 days |

`usage_count` and `last_used_at` track summaries actually injected and details
read; use never changes the content timestamp. `source` records provenance,
not importance. A new `set` can specify `memory_type`, `confidence`, `tags`,
`paths`, `superseded` and `expired`; updates from older clients preserve omitted
metadata. Structured declarations default to `fact`. Learned memories use
`experience`.

The SQLite `memory_index` stores summaries and serialized lexical features,
not full bodies. Writes precompute features; imports and legacy records build
features lazily on first retrieval of that owner. Triggers invalidate entries
on edits and delete them when a record is removed, including older SQL paths.
After indexing, routine retrieval reads summaries/features, and full bodies
remain on demand. The existing bounded lexical cache is retained.

Old databases upgrade additively and transactionally. Existing keys, values,
scopes, owners, provenance and timestamps are preserved. Legacy pinned records
and `preference.*` keys migrate as `preference`; other records default to `fact`,
zero uses, confidence 50, no last use, and active status. Old serialized backup
records receive the same safe metadata defaults; new backup imports preserve
all metadata and rebuild their index lazily.

Its total size, including the wrapper, must fit
`ContextBudget::memory_budget_tokens()` (at most 512 estimated tokens, reduced
for small contexts). Oversized candidates are skipped so smaller relevant
facts can still fit. Sources and timestamps are available in the manager and
tool listing instead of repeated in every injected context. Final request
selection still enforces the overall context budget — see
[context.md](context.md).

`[memory.retrieve]` logs candidate, matched, injected, dropped-for-budget,
token and budget counts without logging fact contents. Retrieval runs at the
start of each user turn. Tool updates are returned to the model immediately;
the injected memory block is refreshed on the following user turn.

## Persistence and resume

The CLI checkpoints each complete user message, assistant response and tool
result before execution advances. A persistence failure stops the turn.
Streaming text that has not yet formed a complete response is not
checkpointed. Compression snapshots are saved at the end of the turn — after
raw messages have been saved — and are anchored to the latest persisted
message ID.

Resume queries messages after the snapshot watermark in backward pages of
128, stopping once enough recent history has been read for the token budget
and a user-turn boundary is available. Persistent agent state is restored
separately for legacy summaries. One exceptionally large latest turn can
still require multiple pages. The initial UI transcript also loads only the
latest page; full-history inspection remains a separate action under
`/memory`.

An assistant tool call saved just before a crash may lack its result. Resume
appends an explicit interrupted-result marker. It does **not** replay the
tool: external side effects may already have happened. The model is told to
inspect current state before retrying.

## User controls

- **`/memory`** — opens Global, Project and Session views. Select a fact to
  inspect its value preview, source, update age and inclusion setting. Edit
  opens a text editor; Alt+Enter saves, Esc cancels. Delete removes that fact
  in its current scope. An edit rejected because of validation or a concurrent
  update is reported instead of overwriting the current record; close and
  reopen the record to load a newer baseline.
- **Session actions** — also expose original history, the current compression
  summary, manual compaction (`/compact`) and clearing Session facts.
- **Deleting a session** removes its Session facts, messages and summary
  through the existing session-deletion path.

## Reference

Low-frequency [Evolution](evolution.md) can additionally learn Project facts
from recurring Experiences. It uses existing validation and optimistic writes,
requires quoted user evidence, marks provenance as `evolved`, and never
overwrites a user-authored fact. Ordinary turns do not gain a learning model call.

| Concern | Code |
|---|---|
| Storage repository, scopes, resume | `crates/memory/src/` (`store.rs`, `session.rs`, `message.rs`, `context.rs`, `events.rs`), `crates/memory/src/scoped.rs` |
| Memory tool and context injection | `crates/cli/src/memory_tool.rs`, `crates/cli/src/memory_context.rs` |
| Fact views and editor | `crates/cli/src/tui/commands/memories.rs`, `crates/cli/src/tui/bottom_pane/memory_editor.rs` |
| Project identity | `crates/cli/src/project_identity.rs` |
| Session restore | `crates/cli/src/session_restore.rs` |
