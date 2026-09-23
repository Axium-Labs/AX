# `/resume`

Open a previous AX session.

## Implementation

1. `execute_slash` asks the memory store for up to 50 sessions and opens `SessionPicker`.
2. Selecting a session calls `ReplState::open_session`.
3. The loader restores that session's saved effective context when available, then adds messages after its compression watermark. Older database versions fall back to the session summary plus uncompacted messages.
4. `select_context` bounds the restored model context using the active model's budget. The transcript UI separately restores the complete stored message history.
5. The selected session becomes current, the runtime is rebuilt on demand, and session-scoped permission decisions are reset.

If no sessions exist, AX reports that there are no previous sessions.

## Code

- Picker and selection: `crates/cli/src/tui/commands.rs` (`open_session_picker`, `open_session`)
- Session restoration: `crates/cli/src/main.rs` (`ReplState::open_session`)
- Token-aware restoration: `crates/core/src/context.rs` (`select_context`)
