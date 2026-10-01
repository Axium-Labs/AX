# 0007. Installation-owned persistent state

Status: accepted

## Context

Workspace-local `.ax` databases and event logs are lost when a user cleans up
the workspace. AX state should follow the installed executable.

## Decision

Default AX home is `.ax` beside the executable. Project stores use canonical-path
SHA-256 keys under `projects/` and retain UUID fact ownership. Existing workspace
IDs are read without creating new workspace metadata. AX_HOME and --data-dir
remain explicit overrides. Legacy stores are copied using a SQLite snapshot with
the writer lock held while copying JSONL; original data is retained.

## Consequences

Changing cwd and deleting workspace contents cannot delete default persistent
state. Installation directories must be writable. A different path without a
legacy ID is a separate project; portable export/import transfers its data.
Uninstalling the entire installation directory deletes its state as well.
