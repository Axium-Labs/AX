# `/new`

Start a fresh session.

## Implementation

1. `execute_slash` calls `ReplState::reset_new_session`.
2. The active session reference, loaded messages, active skill set, and runtime are cleared; session permissions are reset.
3. The UI resets its transcript metadata and displays “New Session”.
4. The SQLite session row is created lazily when the next prompt is submitted, using that prompt to derive its title.

The previous session remains stored and can be reopened with `/resume`.

## Code

- Dispatch: `crates/cli/src/tui/commands.rs` (`execute_slash`)
- State reset and lazy creation: `crates/cli/src/main.rs` (`ReplState::reset_new_session`, `ensure_session`, `create_session`)
