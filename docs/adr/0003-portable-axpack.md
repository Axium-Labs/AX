# 0003. Versioned portable AX data packages

Status: accepted

## Context

SQLite rows and machine-specific project paths are unsuitable as a long-lived
backup format. Session events also live in JSONL files, while credentials and
cache data share the AX data directory with user data.

## Decision

Export selected user data through the memory repository as a versioned `.axpack`
ZIP with a manifest, per-entry SHA-256 checksums and portable JSONL records.
Session messages use ordinals rather than SQLite row IDs. Project facts retain
their existing scope and are rebound to the destination project's UUID on
import. Import validates the complete package before writing, reports a
read-only dry run, and merges without replacing existing session IDs or memory
keys. The CLI is the composition root; no agent tool or parallel database path
is introduced.

## Consequences

The format can evolve independently of SQLite migrations. A target project's
path and identity can differ from the source machine. AX currently accepts
format version 1 only and rejects unknown versions. SQLite changes across the
project and global databases are transactional during normal execution, but
SQLite WAL cannot guarantee crash atomicity across attached database files;
session JSONL installation also has a small crash window. These limits are
documented in the backup guide.
