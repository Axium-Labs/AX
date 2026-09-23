# `/permissions`

View and change the runtime's tool capability policies.

## Implementation

1. `execute_slash` opens a manager showing the current decision for each capability: shell, filesystem read/write, network, MCP, and process launch.
2. Selecting a capability opens its policy choices: Allow, Ask, or Deny.
3. Selecting a choice updates the shared `PermissionStore`, refreshes the manager, and reports the new policy.
4. The runtime's approval policy reads the same store, so updates affect subsequent tool approval checks.

Session-scoped temporary permissions are reset when starting or resuming a session.

## Code

- Registry and views: `crates/cli/src/tui/commands.rs` (`execute_slash`, `permissions`, `permission_items`)
- Policy selection: `open_surface_detail`, `apply_modal_action`
- Approval use: `crates/cli/src/tui/mod.rs` (`ApprovalPolicy` implementation)
- Shared store: `crates/cli/src/main.rs` (`PermissionStore`)
