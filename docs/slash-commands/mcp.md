# `/mcp`

Inspect configured MCP servers, their runtime status, and discovered tools.

## Controls

- Type to search server names, status, or transport.
- Enter opens status, configured protocol, last error, and cached tool descriptions and schemas.
- Alt+C connects and discovers tools.
- Alt+X disconnects and clears cached tools/errors; future use can reconnect lazily.
- Alt+R disconnects, reconnects, and rediscovers tools.
- Esc returns.

Modified shortcuts allow ordinary letters to remain searchable and preserve Ctrl+C as AX's exit shortcut.

## Implementation

`commands/catalogs.rs` reads `McpManager::statuses()` without connecting. `manager.rs` records discovery and call failures, clears errors after successful operations, and caches discovered metadata. Disabled, sleeping, connected, and error states reflect actual configuration and runtime observations.

Successful explicit discovery installs proxies into the tool registry. Disconnect/restart invalidates the agent runtime so the next request gets updated tools. Connection failures are shown in the transcript and manager while leaving the UI open. Each operation uses existing MCP request timeouts. Connection operations currently await completion in the UI action handler.

Configuration still comes from AX's `mcp.toml`; this command does not edit configuration or implement MCP OAuth. The inventory/status interaction is adapted from Codex without importing its server framework.
