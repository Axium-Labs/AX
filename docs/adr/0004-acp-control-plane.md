# 0004. ACP adapter at the AX CLI composition boundary

Status: accepted

## Context

AX's complete turn setup lives in the CLI `ReplState`: model selection, session
restore, memory and skill context, MCP gateway, permissions and checkpointing.
The kernel exposes a reusable loop and events, but a new control plane needs
session-level commands and remote device routing without duplicating this setup.

## Decision

Add an ACP v1 stdio adapter and an outbound Crew device bridge in the AX CLI.
The adapter calls existing `ReplState` and `run_prompt_with` paths. Its own
`ApprovalPolicy` forwards permission requests to the client; cancellation
aborts the current turn and uses AX's existing interrupted-tool recovery on
resume. Crew lives in a separate Rust backend and stores orchestration metadata,
final task outputs, and redacted event indexes. AX retains full sessions,
messages, memory, tools, skills, MCP, models and credentials.

## Consequences

The kernel and storage crates remain unchanged. Existing `ax run`, `ax agents`
and TUI paths keep their original behavior. The adapter currently requires its
process working directory to equal the ACP session `cwd`, so Crew spawns one
AX process in each member directory. Cancellation cannot undo completed tool
side effects; resume marks interrupted calls rather than re-executing them.
Multiple independent Crew tasks can run in parallel through separate ACP
processes, while deterministic dependencies remain Crew's responsibility.


## Workspace discovery extension

_ax/workspace is read-only: optional cwd selects a canonical existing directory;
omission starts at user home. Returns cwd, parent and directories of name/path,
listing directories only. Missing/non-directory paths return invalid params.
Strict sandbox mode confines canonical cwd, listed destinations (including
symlinks) and parent to the process workspace. _ax/capabilities.workspace
advertises the method and read-only property.

Crew heartbeats include registered AX roots under capabilities.workspaces as
id/name/path. This enables project selection without relaxing the bridge
requirement that execution opens a registered root. Session cwd matching stays
unchanged.
