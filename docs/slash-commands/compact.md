# `/compact`

Request immediate compaction of the active session's effective context.

## Implementation

1. `execute_slash` reports the current estimated context size and removes injected retrieved-memory context from the runtime.
2. If a runtime exists, it calls `AgentKernel::compact_now`; otherwise no compression runs.
3. The kernel uses the same layered pipeline as automatic compaction: tool-output cleanup, deduplication, then semantic compression when needed.
4. On success, the CLI stores the effective context snapshot and session summary. Original SQLite messages are retained.
5. The UI reports before and after token estimates, or says there was nothing to compact.

Automatic pressure checks run separately before model requests. `/compact` is the user-triggered path.

## Code

- Dispatch and persistence: `crates/cli/src/tui/commands.rs` (`execute_slash`)
- Compression pipeline: `crates/core/src/lib.rs` (`AgentKernel::compact_now`, `compress`)
- Session snapshot storage: `crates/memory/src/lib.rs` (`save_effective_context`)
