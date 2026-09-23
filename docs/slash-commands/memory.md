# `/memory`

Browse and manage AX's session, project, and global memory facts.

## Implementation

1. `execute_slash` opens the Memory manager with Session, Project, and Global scopes.
2. Selecting Session opens a manager with actions to inspect session facts, the current context summary, and stored messages; run `/compact`; or clear session facts.
3. Clearing session facts deletes only scoped facts for the current session and removes injected retrieved-memory context from the active runtime. Conversation history remains stored.
4. Selecting facts in any scope opens a searchable list. Selecting a record shows its value, source, update age, and inclusion setting, with Edit and Delete actions.
5. In the editor, Alt+Enter saves and Esc cancels. Updates compare the previous value to prevent overwriting concurrent changes. Storage validation applies to UI edits and model writes alike.

Natural language memory requests use the main model's `memory` tool. Explicit unscoped `remember key=value` declarations are session-local. See [Memory behavior](../memory.md) for scope, identity migration, retrieval budgets, and interruption recovery.

## Code

- Registry and root: `crates/cli/src/tui/commands.rs` (`execute_slash`, `memory_root`)
- Fact views and mutations: `crates/cli/src/tui/commands/memories.rs`
- Editor: `crates/cli/src/tui/bottom_pane/memory_editor.rs`
- Storage and retrieval integration: `crates/cli/src/memory_context.rs`
