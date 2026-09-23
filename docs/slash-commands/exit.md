# `/exit`

Exit the AX TUI.

## Implementation

1. The command is registered as a direct action in `SLASH_COMMANDS`.
2. `execute_slash` returns `false` for `/exit`.
3. The TUI submission loop treats `false` as a signal to break its outer event loop and shut down.

## Code

- Registry and dispatch: `crates/cli/src/tui/commands.rs` (`SLASH_COMMANDS`, `execute_slash`)
- Shutdown handling: `crates/cli/src/tui/mod.rs` (slash submission path)
