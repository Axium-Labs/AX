# Storage

AX uses a hybrid storage architecture combining SQLite, JSONL and JSON: SQLite
for fast metadata queries and indexes, JSONL for a complete immutable event
stream per session, and JSON for lightweight configuration. Raw history is
never lost; SQLite is a rebuildable index.

## Data directory layout

AX stores persistent state under `.ax` beside the running executable, or
under `AX_HOME` when explicitly configured. Working directories contain user
project files and optional user-authored skills, not default runtime state.
`--data-dir` remains an explicit override for a selected project store.

```text
<install-dir>/
├── ax(.exe)
└── .ax/
    ├── auth.json
    ├── config.json
    ├── memory.sqlite3              # Global facts
    ├── session-projects.json       # cross-project session picker
    ├── models/
    ├── skills/
    ├── mcp.toml
    └── projects/<project-key>/
        ├── project.json            # Project UUID
        ├── memory.sqlite3          # Project/Session facts and metadata
        ├── memory.sqlite3-wal
        ├── memory.sqlite3-shm
        ├── sessions/<uuid>.jsonl    # authoritative raw history
        ├── mcp.toml                # optional project override
        └── evolution/              # learned skills and Experiences
```

The project key is SHA-256 of the canonical project path (case-normalized on
Windows). Different projects retain separate stores. Project UUIDs remain the
owners of facts. Legacy workspace identities are accepted and copied into the
installation store. Deleting workspace files leaves stored identity and history
intact. A new path without a legacy ID is a new project; use portable export/import
when deliberately moving a project to another path or installation.

At startup, AX copies the old user home and registered project stores, and copies
the current project's legacy `.ax` store on first use. Existing destination files
are preserved, sources are never removed, and completed copies are marked to avoid
repeated scans. A SQLite writer lock protects a database snapshot and JSONL copy;
the backup API includes committed WAL writes. Database publication follows JSONL.
An unwritable installation returns an error rather than falling back to the cwd.
Explicit `AX_HOME` skips old user-home migration; explicit `--data-dir` is retained.

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

### Legacy project identity

AX reads installation `project.json` first, then legacy workspace `project.json`
or `project-id`. It publishes the UUID atomically in the installation-owned project
store and retains the legacy files. No new metadata is written into the workspace.

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

- For portable backups, use `ax export backup.axpack`; see [backup.md](backup.md).
- For low-level disaster recovery on one installation, keep the database and
  its JSONL event streams together.
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

## Durable task queues

Queue checkpoints are ordinary `AgentState` messages prefixed `[ax-task-queue]`.
They include `goal_id`, optional `parent_goal_id`, queue lifecycle state, original
goal, ordered tasks, failure reasons, recovery attempts and cached final response.
Pending/running entries determine remaining work. The existing JSONL-first path
and agent-state index persist this data without a new database schema.

`latest_agent_state(session, prefix)` retrieves the latest state independently of
history pages and compaction. Loading a session restores metadata but does not
implicitly resume it: a new user goal supersedes an active/suspended queue and
archives it under `[ax-task-queue-archive]`, then writes a fresh goal state head.
Explicit resume requires the saved goal ID. Legacy snapshots acquire a stable ID
when restored. Blocked/cancelled/completed states stay terminal across reconnect;
execution budgets suspend the goal. ACP user cancellation waits for the aborted
prompt task before persisting cancellation, preventing late checkpoints from
resurrecting the queue. Disconnect alone preserves resumable work.

Full state and archive messages are excluded from model-visible context. Provider
changes preserve queue metadata. Interrupted tool calls retain existing recovery
placeholders and do not replay side effects.

### Child session persistence

Queued tasks optionally contain a `child` descriptor (`goal_id`, `session_id`,
`cwd`, `state_dir`, `memory_scope`, execution budget). Identity is checkpointed in the controller queue before
child execution. Each child owns `state/child.sqlite3` and its separate
JSONL session stream. Raw model/tool messages are append-only there and do not
inflate controller queue snapshots. Global/project/session memory tool operations
all bind to this child store and its unique owners.

`[ax-child-outcome]` is a terminal receipt saved before advancing the controller.
A reconnect with a running descriptor loads the same workspace and session;
a receipt returns the recorded result without another child model/tool call, even
after its workspace has been removed.
A durable final assistant message also recovers this receipt when interruption
occurs just before receipt writing. Pending children get fresh identities;
completed/failed entries are never restarted. Controller stores/session streams
are excluded when copying workspace files.

`state/workspace.json` records the owned cwd, Git repository, lifecycle state and
last checkpoint time. `state/workspace.lock` is held for the prepared child lifetime;
interruption releases it and marks an unfinished child `interrupted`. Terminal
receipt writing precedes manifest terminal state and workspace cleanup. GC retries
terminal cleanup, expires unleased idle workspaces, and retains the state directory.
Legacy descriptors without `state_dir` move the old `.ax` store and JSONL together
before enabling cleanup. Persistent memory and shell AX_HOME now bind to `state`,
not a directory inside the disposable cwd. History loads in chronological order;
checkpoints append only newly generated raw messages during resume.

ACP `session/load` also replays the tools from child sessions referenced by raw
queue checkpoints and archives, including cancelled or interrupted children.
Each child is replayed once with the same `child-session:call-id` identity used
by live events. Arguments, completed results and unfinished calls remain visible
after cancellation and reconnect, even after workspace cleanup. Child prompts
and internal assistant text remain private to the child transcript.
