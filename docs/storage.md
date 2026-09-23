# Storage

AX uses a hybrid storage architecture combining SQLite, JSONL and JSON: SQLite
for fast metadata queries and indexes, JSONL for a complete immutable event
stream per session, and JSON for lightweight configuration. Raw history is
never lost; SQLite is a rebuildable index.

## Data directory layout

AX keeps **project state** in the project's `.ax` directory and **user state**
in the AX home (`~/.ax`, or `$AX_HOME` when set).

```text
project-root/
└── .ax/
    ├── memory.sqlite3          # SQLite database (indexes, metadata, facts)
    ├── memory.sqlite3-shm      # WAL shared memory
    ├── memory.sqlite3-wal      # WAL log
    ├── project.json            # portable project identity (UUID)
    └── sessions/               # JSONL event streams
        ├── <session-uuid>.jsonl
        └── <session-uuid>.jsonl

~/.ax/
├── auth.json                   # provider credentials (0600 on Unix / icacls on Windows)
├── config.json                 # last model selection (legacy config.toml auto-migrated)
└── models/                     # model catalog cache
    ├── deepseek.json
    ├── openai.json
    ├── openai-codex.json
    └── pi-catalog.json
```

## SQLite schema

The database (`memory.sqlite3`, WAL mode, `PRAGMA user_version = 6`) contains:

| Table | Purpose | Key columns |
|---|---|---|
| `sessions` | Session metadata | `id` (PK), `title`, `created_at`, `updated_at` |
| `messages` | Message index | `id` (PK), `session_id` (FK, cascade), `role`, `kind`, `content`, `metadata`, `created_at`, `event_offset`, `event_length` |
| `agent_states` | Persistent system instructions/skills, preserved across compression | `message_id` (PK, FK), `session_id`, `content`, `metadata`, `created_at` |
| `session_summaries` | One row per session: cumulative summary + compression watermark | `session_id` (PK), `content`, `compressed_message_count`, `updated_at`, `through_message_id` |
| `long_term_memory` | Legacy flat memory records | `id` (PK), `key` (unique), `value`, `category`, `created_at`, `updated_at` |
| `scoped_memories` | Global / Project / Session facts | `scope`, `owner`, `key` (composite PK), `value`, `source`, `updated_at`, `always_include` |
| `memory_migrations` | Migration markers for scoped facts | `category` (PK), `migrated_at` |
| `project_memory_migrations` | Path-owner → UUID migration markers | `owner` (PK), `project_id` |

Notes:

- The message **content** in `messages` is cleared (`content=''`,
  `metadata='null'`) once an event has been written to JSONL; the JSONL event
  is the source of truth. Columns `event_offset` / `event_length` point into
  the session's JSONL file.
- Indexes exist on `sessions(updated_at DESC)`, `messages(session_id, id
  DESC)`, `long_term_memory(category, updated_at DESC)` and
  `agent_states(session_id, message_id)`.
- The schema evolves by additive migrations guarded by `PRAGMA user_version`.

## JSONL event streams

One file per session, `sessions/<session-uuid>.jsonl`; each line is a complete
`StoredMessage` JSON object:

```json
{"id":1,"session_id":"...","role":"user","kind":"message","content":"user input","metadata":null,"created_at":1234567890}
{"id":2,"session_id":"...","role":"assistant","kind":"message","content":"AI response","metadata":{"tokens":150},"created_at":1234567891}
```

JSONL is **append-only** (crash-safe), complete (including compressed
messages), easy to back up, and auditable independently of SQLite.

## Write path

`append_message` runs inside an `IMMEDIATE` transaction:

1. Recovery check (`sync_session_locked`) — ensure the index is up to date.
2. Insert an index placeholder in `messages` to obtain the new ID.
3. Build the complete `StoredMessage`.
4. Append the event to JSONL and `fsync` (atomic, O(1)).
5. Update the index pointer (`event_offset`, `event_length`).
6. If the kind is `AgentState`, store it separately in `agent_states` (so it
   survives compression).
7. Update the session's `updated_at` timestamp.
8. Commit.

**Ordering is deliberate: JSONL first, SQLite after.** The event can never be
lost; the index can always be rebuilt.

## Read path

`load_messages` first runs the recovery check, then queries the index with
`LIMIT`/`OFFSET` pagination and decodes each message:

- Indexed rows (have `event_offset`/`event_length`): seek into the JSONL file
  and read exactly `length` bytes, then verify `event.id` / `event.session_id`
  match the index.
- Legacy rows (pre-migration): decode inline from SQLite.

## Crash recovery

`sync_session_locked` runs before every read/write and synchronizes the index
with the JSONL file:

1. Count unindexed messages and the indexed end position.
2. Compare with the JSONL file size.
3. Fast path: everything synchronized → return.
4. Corruption check: JSONL smaller than the indexed end → error
   ("missing JSONL events").
5. Otherwise scan the JSONL, rebuild missing index entries, and restore
   `agent_states`.
6. Migrate legacy SQLite-inline messages: read content, append to JSONL,
   update the pointer, clear the content fields.

Crash scenarios:

| Crash timing | JSONL state | SQLite state | Recovery result |
|---|---|---|---|
| Before JSONL append | unchanged | unchanged | message lost (expected; transaction not committed) |
| After `fsync`, before index | complete event | index missing | `sync_session_locked` rebuilds index |
| After index, before commit | complete event | rolled back | next write re-indexes (idempotent) |
| After commit | complete event | complete index | normal |

## Migration

### Old inline SQLite messages → JSONL

Happens automatically and per-session on first access. Old messages
(`event_offset IS NULL`) are read from SQLite, appended to JSONL, and their
index pointers updated; SQLite content fields are cleared afterwards. No data
is lost.

### Project identity `project-id` → `project.json`

`project_identity::load_or_create` migrates automatically:

1. Read `project.json`; return if present.
2. Else read the legacy `project-id` text file.
3. Else generate a new UUID.
4. Write `project.json` atomically (temp file + hard link).
5. Delete the old `project-id` file.

### Legacy `config.toml` → `config.json`

`AxConfig::load_from_home` reads `config.json`; if absent, it parses the
legacy `config.toml`, saves the result as `config.json`, and continues.

## Consistency & concurrency

- **Single process, multi-threaded**: `IMMEDIATE` transactions +
  `busy_timeout(5s)` serialize writers.
- **Multi-process**:
  - JSONL: append-only, naturally safe.
  - SQLite: WAL mode + `IMMEDIATE` transactions queue automatically.
  - Risk: two processes generating different `id`s — avoided by the
    transaction lock.
- **Read-time validation**: `decode_indexed` errors when the JSONL event does
  not match the SQLite index (`event.id`, `event.session_id`).

## Performance

- **Write**: JSONL append O(1) + SQLite index O(log N); ~1–2 ms/message
  including `fsync`.
- **Read**: paginated query O(log N + K); JSONL random read O(1) via seek;
  only requested messages are loaded.
- **Overhead**: ~100–200 bytes/message of SQLite index + actual message size in
  JSONL; roughly 1.1–1.2× a pure-SQLite layout, in exchange for an audit trail
  and crash safety.
- **Compaction**: only the latest N messages are loaded; a 1000-message,
  500 KB session loads the latest 50 (~95% less JSONL read).

## Backup & troubleshooting

- Back up `.ax/memory.sqlite3` **and** `.ax/sessions/` together (e.g. `tar` /
  zip the whole `.ax`).
- Old and new AX versions should not run against the same data directory:
  old versions ignore JSONL and can create inconsistencies.
- Do not edit JSONL files manually — the SQLite index uses exact offsets.
- If a JSONL file is corrupted or deleted, restore from backup, or delete the
  session and start fresh; migration retries from SQLite on next access.
- After confirming migration, `VACUUM;` on the SQLite file reclaims space.

## Reference

| Concern | Code |
|---|---|
| Storage repository, sync, schema | `crates/memory/src/lib.rs` |
| Scoped memories | `crates/memory/src/scoped.rs` |
| Project identity migration | `crates/cli/src/project_identity.rs` |
| Config migration | `crates/cli/src/config.rs` |
