# JavaScript Mods

Mods are executable session extensions, inspired by the local Deepseek-harness
`register(on, options)` bridge. AX discovers them through its existing scoped
registry; the CLI composition root lazily starts a Node worker when a prompt
has enabled Mods. Catalog queries, installation and toggles never evaluate JS.
Core owns only the optional `RuntimeExtension` boundary, with no Node dependency.

## Package and management

Install a directory containing `mod.json` and its compiled JavaScript entry:

```json
{
  "name": "notes",
  "description": "Add project context and a local command",
  "version": "1.0.0",
  "entry": "index.mjs",
  "enabled": true,
  "userConfig": { "note": "Run the project checks before finishing." }
}
```

The entry must be a relative `.mjs` or `.js` file inside the package. Use `.mjs`
or an ESM `package.json` for ES imports in `.js`. Compile TypeScript first.
`userConfig` values accept strings, numbers, booleans and string lists; manifest
values override defaults supplied to `defineMod`. Invalid manifests fail with
their package path. Installation rejects symlinks and existing destinations,
copies through a staging directory and does not overwrite other packages.

```bash
ax capabilities mods add notes --scope project --source /packages/notes
ax capabilities mods list --scope project
ax capabilities mods disable notes --scope project
ax capabilities mods enable notes --scope global
ax capabilities mods remove notes --scope project
```

Global packages live at `$AX_HOME/mods/<name>`; project packages live at
`<project>/.ax/mods/<name>`. `[mods]` / `[mods.overrides]` in `config.toml` use
the same override/mask policy as Skills/MCP/Agents. Removing or disabling an
inherited global Mod in Project only masks it there. `/mods` and `/settings`
provide a TUI manager. Crew's Plugins page supports add, search, details, scope,
enable/disable and remove. Edit `userConfig` in the installed `mod.json`.
ACP exposes `_ax/mods` and `_ax/scopedCapabilities` with `kind: "mods"`; catalog
rows contain source, version and userConfig in addition to scope/status.

## Hooks, commands and tools

`index.mjs` can export `register`, a default specification with `register`, or
use `defineMod` from `@ax/mods`. The Deepseek-harness package import name is
also mapped to this small `defineMod` shim; other dependencies must be installed
in the package before importing it.

```js
import { defineMod } from '@ax/mods'
export default defineMod({
  name: 'notes',
  userConfig: { note: 'Check the project.' },
  register(on, options) {
    on('session.start', async ($, event, next) => {
      await $.command.register({ name: 'note', description: 'Show project note' })
      await $.tool.register({
        name: 'note', description: 'Read the configured project note',
        inputSchema: { type: 'object', properties: {} },
      })
      return next(event)
    })
    on('prompt.submit', async ($, event, next) =>
      next({ ...event, context: [...event.context, options.note] }))
    on('command.run', { command: 'note' }, async () => ({ text: options.note }))
    on('tool.call', { tool: 'mcp__notes__note' }, async () => ({ result: options.note }))
  },
})
```

Run `/mod:note` in CLI/TUI/Crew chat. A command returns its result without a model
call, persists the result and completes its goal. Tools registered during
`session.start` enter the actual model tool schema as `mcp__<mod>__<tool>` and
execute through the normal permission boundary. Unsupported or duplicate names
are rejected. Register commands/tools at session start, before the schema freezes.

Supported events: `session.start`, `session.end`, `turn.start`, `prompt.submit`,
`tool.call`, `command.run`, `turn.complete`. Matchers accept exact values,
value lists and RegExp. Hooks must return a result or `next(event)`; repeated
`next` calls reuse the same promise and never execute the underlying tool twice.
`.catch(handler)` can recover a hook failure. `prompt.submit` may change model
input and add system context; raw user history is preserved. `tool.call` may
observe output, replace it or return `{deny: "reason"}`. Changing already-approved
tool arguments via `next` is rejected; issue a new `$.tool.call` for a separate
permission check. `turn.complete` is an observation event: its return value does
not rewrite an already-streamed answer.

## State and host operations

`$.state.get/set/delete` keeps ephemeral per-session values; `get` returns
`{value}`. A cached worker preserves these across ACP runtime reconstructions
in the same live session. `$.store.get/set/delete` persists per-Mod JSON in
`$AX_HOME/mod-store/<name>.json`, limited to 4 MiB with atomic file replacement.
Concurrent session writes are last-writer-wins, not transactions. Removing a
package preserves its store. Session ID/cwd/model/turn count/effective messages
are exposed by `$.session`; usage reports the known context window, without
inventing token counts or account rate limits.

`$.tool.call` invokes base host tools with a fresh permission/profile check.
`$.fs.write` uses the filesystem tool; `$.http.fetch` uses the web tool's fetch
operation. Workspace reads (`read/exists/list/stat`) reject canonical paths
outside the project and limit file reads to 4 MiB. `$.clock` supports now/sleep/
after/every; session disposal clears scheduled timers. `$.env` reads/writes the
worker environment. UI log/toast/status go to stderr; no stdout pollution enters
the ACP protocol. `ui.open` reports `isPlaced:false`, and no UI panes are rendered.

This is an AX Mod API, not complete DSH/Claude SDK compatibility: `ui.render`,
`ui.ask`, managed tiers (`next.to`), programmatic `prompt.submit`, process APIs
and other events/namespaces fail explicitly. Host tool lookup captures the
initial registry; later per-turn memory/skill tools cannot be called through
`$.tool.call`. Parent Mods are not copied into isolated child kernels. Direct
imports can still use ordinary Node APIs; the helper boundaries are not a JS sandbox.

## Execution requirements and lifecycle

Node.js 20.6+ must be on PATH, or set `AX_MOD_NODE` to its executable. No enabled
Mods means no Node process. Mods are trusted executable code, and loading is
rejected when AX workspace sandbox mode is not off or when a local runtime is
executing in an SSH context. Install/run remote Mods on the remote AX host.

Each hook has a 10-second budget, catch handlers 1 second, and a whole bridge
event 15 seconds. Errors/timeouts are surfaced; failing workers are terminated.
Cancelled events are never replayed, and side effects already performed cannot
be rolled back. Start a new session after a failed worker on a retained local
kernel. ACP sessions rebuilt on the next turn replace failed workers.
Manifest/config/enable/entry changes trigger reload at the next prompt; imported
dependency changes require a new session. Reload resets ephemeral state and tool
registrations, while persistent stores remain. Workers dispose on new/deleted
sessions and process exit; abrupt termination cannot guarantee `session.end`.
