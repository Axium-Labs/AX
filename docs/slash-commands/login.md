# `/login`

Connect a model provider by starting an authentication flow.

## Implementation

1. `execute_slash` opens an authentication method manager with account sign-in and API key options.
2. Selecting a method lists providers whose declared authentication type supports it.
3. Selecting an API key provider opens `SecretInput`; the submitted key is stored through `AuthStorage` in AX's auth file.
4. Codex OAuth starts a background device-code flow. The UI receives the verification prompt and completion status through `LoginUpdate` messages.
5. Other provider-owned OAuth or ambient credential flows display guidance rather than changing credentials. After saving an API key, AX invalidates the runtime and refreshes model catalogs asynchronously.

The command does not directly select a model. Use `/model` after authentication.

## Code

- Registry and entry point: `crates/cli/src/tui/commands.rs` (`SLASH_COMMANDS`, `execute_slash`, `open_provider_login`)
- Follow-up selections: `apply_modal_action`, `open_login_provider_list`, `start_codex_login`
- Secret entry: `crates/cli/src/tui/bottom_pane/secret_input.rs`
