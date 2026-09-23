# Portable backup and restore

AX exports user data through its storage repository. Commands:

    ax export backup.axpack
    ax export backup.axpack --memory
    ax export backup.axpack --sessions
    ax import backup.axpack --dry-run
    ax import backup.axpack

No flag on export selects both memory and sessions. Passing both selection flags
also selects both. Export requires a new destination file; it never overwrites
an existing archive. Import prints planned or completed counts and conflict
lists as JSON. Dry-run validates the entire archive, opens existing databases
read-only, and does not create a project identity or database.

## Format

An axpack is a ZIP archive with these exact entries:

| Entry | Contents |
|---|---|
| manifest.json | format axpack, version 1, AX version, creation time, included data types, SHA-256 and record count for each data entry |
| metadata.json | source project's portable UUID |
| memories.jsonl | scoped global, project, and session facts |
| legacy_memories.jsonl | legacy flat memory records with source store (global or project) |
| sessions.jsonl | session IDs, titles and timestamps |
| messages.jsonl | complete raw messages in session order, with portable ordinal numbers rather than SQLite IDs |
| summaries.jsonl | summary content, message ordinal watermark and optional effective-context snapshot |

The package has no SQLite database, WAL file, JSONL event offsets or absolute
project path. Import verifies every entry's SHA-256, size, record count,
references, IDs and format version before writing. Version 1 is supported;
unknown versions fail explicitly, leaving room for future format migrations.
The current reader limits each uncompressed entry to 128 MB and the total to
512 MB.

## Included data and boundaries

Default export includes sessions, raw messages, summaries and effective
snapshots, scoped global/project/session facts, and legacy flat memory. It
reads the current project database and the AX home global memory database.
Invalid or credential-like memory facts that fail the existing memory
validator are omitted.

It never packages auth.json, API keys, OAuth tokens, MCP configuration or
credentials, provider config, model caches, temporary files, language-server
process state, or a whole AX data directory. Conversation messages are
user-authored data and may themselves contain sensitive text or tool output.
Review the destination archive before sharing it; AX does not attempt to
redact arbitrary conversation history.

## Merge and project identity

Import preserves existing data. A matching session ID skips that whole
incoming session, including its messages, summary and session-scoped facts.
An existing scoped memory key or legacy memory key is kept; the incoming value
is skipped. A memory-only package cannot restore session facts without the
associated session in the same package; those facts are reported as orphan
session memories and skipped.

Project facts in the package carry the source project's UUID, not its old
path. Import maps them to the current project's UUID from .ax/project.json;
it does not overwrite that identity. Export first applies AX's existing
path-owner migration when an old project database needs it. Import creates
the current project identity only for an actual merge, never for dry-run.

Import stages new session JSONL events, inserts SQLite indexes and both
project/global facts under a database transaction, then publishes events
before committing. On a reported failure it rolls back the database and
removes staged files. SQLite WAL transactions spanning the attached project
and global databases are not guaranteed to be crash-atomic across both files.
A process crash during the narrow event-publication window can leave an
unindexed orphan JSONL file; AX will not silently overwrite that file on a
retry. The existing JSONL events remain the source of truth for normal
session operation.

## Code

The CLI routes to memory::backup::{ExportService, ImportService}. The
service uses the existing MemoryStore to read sessions and messages and
rebuilds local SQLite message IDs and JSONL offsets on import. It does not
register an Agent Tool.
