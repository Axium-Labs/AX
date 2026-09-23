# `/logout`

Remove credentials stored in AX for a selected provider.

## Implementation

1. `execute_slash` reads provider IDs from AX's `AuthStorage` and opens a provider picker.
2. If no stored credentials exist, it reports that there is nothing to remove.
3. Selecting a provider removes its AX credential and invalidates the active runtime.

Logout does not modify environment variables or credentials managed by a provider's own CLI. Providers using ambient credentials may appear only if AX has a stored entry for them.

## Code

- Registry and picker: `crates/cli/src/tui/commands.rs` (`SLASH_COMMANDS`, `execute_slash`, `open_provider_logout`)
- Credential removal: `apply_modal_action`, using `model::AuthStorage`
