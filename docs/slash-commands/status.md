# `/status`

Show a snapshot of runtime, model, session, context, budget, environment, and latency information.

## Implementation

1. `execute_slash` builds an info panel from current app and runtime state.
2. The panel includes AX version and mode, selected provider/model, model context window and tool support, current session title and stored message count, displayed context usage, working directory, execution limits, and telemetry aggregates.
3. Latency data comes from the process-local `tool::telemetry::snapshot()` registry.

The command is read-only. The displayed values are a snapshot taken when the panel is opened.

## Code

- Registry and panel: `crates/cli/src/tui/commands.rs` (`execute_slash`, `status_panel`)
- Telemetry: `crates/tool/src/telemetry.rs`
