# AX Hybrid Storage Architecture

AX uses a hybrid storage approach combining SQLite, JSONL, and JSON to achieve efficient metadata queries, complete event sourcing, and clean configuration management.

## Architecture Overview

```
AX
├── SQLite (memory.sqlite3)
│   ├── Session metadata (id, title, timestamps)
│   ├── Message index (id, session_id, role, kind, event_offset, event_length)
│   ├── Agent State (persistent system instructions and skills)
│   ├── Summary / Compress state (watermark, effective_context)
│   ├── Long-term Memory (user-defined memories)
│   └── Scoped Memories (global/project/session scopes)
│
├── JSONL (sessions/<session-id>.jsonl)
│   └── Raw event stream (complete message content: role, kind, content, metadata)
│
└── JSON
    ├── project.json (project unique identifier)
    ├── auth.json (provider credentials: API keys and OAuth tokens)
    └── models/<provider>.json (model catalog cache)
```

## Storage Responsibilities

### SQLite: Fast Queries and Metadata

- **Session metadata**: title, created/updated timestamps
- **Message index**: ID, type, timestamps, JSONL offset/length
- **Agent State**: persistent system instructions (preserved across compression)
- **Compression state**:
  - `through_message_id`: compression watermark
  - `effective_context`: compressed effective context snapshot
  - `content`: summary text
- **Memory**: long-term and scoped memories

**Advantages**:
- Efficient pagination queries (`LIMIT/OFFSET`)
- Complex filtering and aggregation (`JOIN`, `COUNT`, `MAX`)
- Transaction guarantees (ensures index and event stream consistency)

**Limitations**:
- Does not store full message bodies (prevents SQLite bloat)
- Message content left empty (`content=''`, `metadata='null'`)

### JSONL: Immutable Event Stream

One file per session: `sessions/<session-uuid>.jsonl`

Each line is a complete `StoredMessage` JSON object:

```json
{"id":1,"session_id":"...","role":"user","kind":"message","content":"user input","metadata":null,"created_at":1234567890}
{"id":2,"session_id":"...","role":"assistant","kind":"message","content":"AI response","metadata":{"tokens":150},"created_at":1234567891}
```

**Advantages**:
- Append-only, no modifications (crash-safe)
- Complete message history (including compressed messages)
- Easy to backup and restore
- Can be audited and debugged independently of SQLite

**Write flow**:
1. Append JSON line to JSONL file
2. Call `fsync` to ensure persistence
3. Record offset and length in SQLite
4. Commit SQLite transaction

**Recovery mechanism**:
If process crashes after step 2, before step 4:
- JSONL file has complete write (with `\n`)
- SQLite index may be missing or incomplete
- Next read, `sync_session_locked` will:
  - Scan JSONL file
  - Rebuild missing index entries
  - Migrate old SQLite inline messages (if any)

### JSON: Lightweight Configuration and Metadata

#### `project.json`
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000"
}
```
- Project unique identifier
- Supports project renaming and moving
- Atomic write (temporary file + hard link)

#### `auth.json`
```json
{
  "deepseek": {
    "type": "api_key",
    "key": "sk-..."
  },
  "openai-codex": {
    "type": "oauth",
    "access": "eyJ...",
    "refresh": "...",
    "expires": 1234567890,
    "account_id": "org-..."
  }
}
```
- Provider credentials (API keys, OAuth tokens)
- Permission-restricted (Unix `0600`, Windows `icacls`)
- Supports environment variable references

#### `models/<provider>.json`
```json
{
  "saved_at_unix": 1234567890,
  "models": [
    {
      "id": "deepseek-chat",
      "name": "DeepSeek Chat",
      "provider": "deepseek",
      "context_window": 32768,
      "max_output": 8192
    }
  ]
}
```
- Local cache of model catalogs
- Avoids API requests on every startup
- Falls back to cache after 15s timeout

## Key Operation Flows

### Writing a New Message

1. **Recovery check** (`sync_session_locked`): Ensure index is up-to-date
2. **Create index placeholder**: Get new ID from SQLite
3. **Construct complete `StoredMessage`**: Include all content
4. **Append to JSONL**: Atomic operation with `fsync`
5. **Update index pointer**: Store offset and length in SQLite
6. **Store Agent State separately** (if applicable): Preserved across compression
7. **Update session timestamp**
8. **Commit transaction**

### Reading Messages

1. **Recovery check**: Ensure index is current
2. **Paginated index query**: Use `LIMIT/OFFSET` on SQLite
3. **Decode each message**:
   - If indexed (has offset/length): Read from JSONL using `seek`
   - If legacy (pre-migration): Decode from SQLite
   - Verify index consistency

### Crash Recovery

`sync_session_locked` executes before every read/write, ensuring index and event stream are synchronized:

1. **Check index state**: Count unindexed messages, find indexed end position
2. **Get JSONL file size**
3. **Fast path**: If everything synchronized, return immediately
4. **Detect corruption**: If JSONL smaller than indexed, error
5. **Scan JSONL**: Rebuild index for all events
6. **Migrate legacy SQLite messages**: Move inline content to JSONL

### Compression and Summary

Compression does not delete original events in JSONL, only updates the watermark in SQLite:

```rust
// Calculate new watermark (keep latest N messages uncompressed)
let through: i64 = transaction.query_row(
    "SELECT COALESCE(MAX(id), 0) FROM messages WHERE session_id = ?1 AND id NOT IN
     (SELECT id FROM messages WHERE session_id = ?1 ORDER BY id DESC LIMIT ?2)",
    params![session_id, keep_latest],
    |row| row.get(0)
)?;

// Update summary and watermark (don't delete data)
transaction.execute(
    "INSERT INTO session_summaries (...) VALUES (...)
     ON CONFLICT(session_id) DO UPDATE SET
     through_message_id = MAX(session_summaries.through_message_id, excluded.through_message_id), ...",
    params![session_id, summary, compressed_message_count, through]
)?;
```

When reading, only load messages after the watermark:

```rust
// Get compression watermark
let through: i64 = self.connection
    .query_row("SELECT through_message_id FROM session_summaries WHERE session_id = ?1", [session_id], |row| row.get(0))
    .optional()?
    .unwrap_or(0);

// Only load messages after watermark (+ Agent State, regardless of compression)
let mut statement = self.connection.prepare(
    "SELECT ... FROM messages
     WHERE session_id=?1 AND (id>?2 OR (kind='agent_state' AND ?3=0)) ..."
)?;
```

## Migration Paths

### Old SQLite Messages → JSONL

For pre-migration messages (`event_offset IS NULL`):

1. First access, `sync_session_locked` detects `legacy_count > 0`
2. Read complete message content from SQLite (`content`, `metadata`)
3. Append to JSONL file
4. Update index pointer (`event_offset`, `event_length`)
5. Clear content fields in SQLite (`content=''`, `metadata='null'`)

### Project Identity `project-id` → `project.json`

`project_identity::load_or_create` migrates automatically:

1. Try reading `project.json`, return if exists
2. Try reading old `project-id` text file
3. If neither exists, generate new UUID
4. Write `project.json` (atomic operation)
5. Delete old `project-id` file

## Consistency Guarantees

### Write Ordering

1. **JSONL first**: Ensure raw event is persisted
2. **SQLite after**: Index can be rebuilt, events cannot be lost

### Crash Scenarios

| Crash Timing | JSONL State | SQLite State | Recovery Result |
|-------------|-------------|--------------|-----------------|
| Before JSONL append | Unchanged | Unchanged | Message lost (expected, transaction not committed) |
| After `fsync`, before index | Complete event | Index missing | `sync_session_locked` rebuilds index |
| After index, before commit | Complete event | Transaction rolled back | Next write re-indexes (idempotent) |
| After commit | Complete event | Index complete | Everything normal |

### Concurrent Writes

- **Single process, multi-threaded**: `IMMEDIATE` transaction + `busy_timeout(5s)` serializes writes
- **Multi-process**:
  - JSONL: Append-only, naturally concurrent-safe
  - SQLite: WAL mode + `IMMEDIATE` transaction, automatic queuing
  - Risk: Two processes might generate different `id`s (avoided via transaction lock)

### Index Consistency Validation

Validation on read:
```rust
if event.id != raw.0 || event.session_id != raw.1 {
    return Err(MemoryError::InvalidValue("event index does not match JSONL".into()));
}
```

## Performance Characteristics

### Write Performance

- **JSONL append**: O(1), append to file end
- **SQLite index**: O(log N), B-Tree insert
- **Overall**: ~1-2ms/message (including fsync)

### Read Performance

- **Paginated query**: O(log N + K), index lookup + K records
- **JSONL random read**: O(1), direct seek to offset
- **Memory usage**: Only loads requested messages, not full history

### Storage Overhead

- **SQLite**: ~100-200 bytes/message (index + metadata)
- **JSONL**: Actual message size (JSON + `\n`)
- **Total**: ~1.1-1.2x pure SQLite, but independently backupable and cleanable

### Compression Benefits

- **Before compression**: 1000 messages × 500 bytes = 500 KB
- **After compression**:
  - SQLite: 1000 indexes (100 KB)
  - JSONL: 500 KB (unchanged)
  - Summary: 5 KB
  - **Query reduction**: Only load latest 50, reduces 95% of JSONL reads

## File Layout Example

```
project-root/
└── .ax/
    ├── memory.sqlite3          # SQLite database
    ├── memory.sqlite3-shm      # WAL shared memory
    ├── memory.sqlite3-wal      # WAL log
    ├── project.json            # Project identity
    └── sessions/               # JSONL event streams
        ├── 550e8400-e29b-41d4-a716-446655440000.jsonl
        └── 7c9e6679-7425-40de-944b-e07fc1f90ae7.jsonl

~/.ax/
├── auth.json                   # Provider credentials
└── models/                     # Model catalog cache
    ├── deepseek.json
    ├── openai.json
    ├── openai-codex.json
    └── pi-catalog.json         # Complete model catalog
```

## Test Coverage

All tests passing (`cargo test --workspace --lib`):

**Memory crate (13 tests)**:
- ✅ `session_messages_are_paginated_and_cascade_deleted`
- ✅ `effective_snapshot_resumes_only_its_session_and_keeps_raw_history`
- ✅ `long_term_memory_upserts_by_key_and_filters_by_category`
- ✅ `repeated_compaction_and_reopen_preserve_complete_history`
- ✅ `migration_from_old_summary_schema_keeps_rows`
- ✅ `raw_events_live_in_jsonl_and_sqlite_keeps_only_the_index`
- ✅ `legacy_rows_and_unindexed_jsonl_events_recover_without_loss`
- And 6 more scoped memory tests

**Full workspace**: 63 tests passed, 0 failed, 1 ignored

## Summary

AX's hybrid storage architecture achieves a good balance between reliability, performance, and maintainability:

- **SQLite**: Efficient metadata queries and transaction guarantees
- **JSONL**: Complete event sourcing and crash safety
- **JSON**: Clean configuration and portability

Core design principles:

1. **JSONL is the source of truth**
2. **SQLite is a rebuildable index**
3. **Crash recovery is automatic**
4. **History is complete**
5. **Migration is seamless**
