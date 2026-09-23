# `/skills`

Browse, search, and enable or disable installed AX skills.

## Controls

- Type to search names, descriptions, or status.
- Enter shows the indexed source path, description, triggers, required tools, and missing dependencies.
- Space toggles the selected skill; Esc returns.

## Implementation

`commands/catalogs.rs` combines catalog metadata, registered tool names (including the MCP gateway), and explicit choices in `<data-dir>/disabled-skills.json`. Enabled, disabled, and unavailable (missing tools) are distinct states. No instructions are loaded merely to browse.

`skill_settings.rs` persists choices before changing runtime state. Disabling removes the skill's tagged instructions from effective context and prevents future routing. Session restore filters disabled skill instructions. Original stored messages remain intact. Re-enabling permits routing on a later matching turn. Information already incorporated into conversation summaries cannot be selectively removed.

The catalog still uses AX's `skill.toml` and `instructions.md` format. The enable/disable interaction follows Codex's skill manager; it does not import Codex's skill runtime.
