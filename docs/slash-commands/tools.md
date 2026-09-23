# `/tools`

Inspect built-in tools, discovered MCP proxies, and the lazy MCP gateway.

## Controls

Type to search tool names, descriptions, or source (`Built-in` / `MCP`). Enter opens details; Esc returns. Details show the real description and JSON input schema, plus server and remote name for MCP proxies.

## Implementation

`commands/catalogs.rs` reads the same tool registry used by the agent. The `mcp` gateway is always listed. Individual remote tools appear after explicit discovery through `/mcp`; undiscovered tools remain callable through the gateway. Browsing does not connect servers.

Permissions depend on each call's arguments and remain enforced by the existing permission policy. `/permissions` manages those choices. This view does not claim a static permission decision for every possible call.
