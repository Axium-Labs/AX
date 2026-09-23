# `/model`

Choose a configured provider model and, where supported, its reasoning mode. The command accepts an optional model reference, for example `/model provider/model-id`.

## Implementation

1. `execute_slash` opens `ModelPicker` using locally cached model catalogs and the current selection.
2. With a non-empty argument, AX first tries an exact match against the cached snapshot. A unique match is applied directly.
3. Otherwise, the picker opens with the argument as its search text and refreshes configured provider catalogs in the background.
4. Selecting a model with reasoning options opens `ReasoningPicker`; the selected model and effort are then applied.
5. Applying a selection updates provider, endpoint, model ID, context size, tool support, and reasoning effort; it invalidates the current runtime and persists the selection for the next launch.

Only models from configured providers are offered. A discoverable provider without an enabled protocol adapter cannot be selected.

## Code

- Dispatch and picker: `crates/cli/src/tui/commands.rs` (`execute_slash`, `open_model_picker`)
- Match and apply: `find_exact_model`, `apply_model_info`
- Catalog refresh: `crates/cli/src/tui/catalog_refresh.rs`
- Picker UI: `crates/cli/src/tui/bottom_pane/model_picker.rs`
