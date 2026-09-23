# MCP

AX integrates Model Context Protocol servers as ordinary tools. This document
covers configuration, transports, lazy connection behavior, and the `/mcp`
command.

## Overview

The `mcp` crate reads only server **configuration metadata** until a server's
tools are actually needed. It implements three transports — stdio, Streamable
HTTP and WebSocket — with `initialize` negotiation, the modern stateless
protocol, paginated `tools/list`, `tools/call`, request timeouts and process
cleanup.

## Configuration

MCP servers are configured in `mcp.toml` — by default `<data-dir>/mcp.toml`
(the project's `.ax/mcp.toml`), or any path passed with `--mcp-config`.
See `mcp.example.toml` at the repository root:

```toml
[servers.local-files]
transport = "stdio"
command = "your-mcp-server"
args = ["--stdio"]
description = "Local filesystem indexer and search"
capabilities = ["search", "read"]
protocol_version = "2026-07-28"
request_timeout_secs = 30

[servers.remote]
transport = "http"
url = "https://example.com/mcp"
headers = { Authorization = "Bearer replace-me" }
description = "Project issue tracker"
capabilities = ["issues", "tasks"]
enabled = false
```

| Field | Meaning |
|---|---|
| `transport` | `stdio`, `http` or `websocket` |
| `command` / `args` | Process to spawn (stdio transport) |
| `url` | Server endpoint (http / websocket) |
| `headers` | Static request headers |
| `description` | Shown to the model through the capability catalog *without* connecting |
| `capabilities` | Declared capabilities, also visible without connecting |
| `protocol_version` | MCP protocol version to negotiate |
| `request_timeout_secs` | Per-request timeout |
| `enabled` | Default `true`; `false` keeps the server defined but dormant |

`description` and `capabilities` are exposed to the model before any
connection exists, so the model can discover what a sleeping/disabled server
offers before spending a real connection on `list_tools`/`call`.

### LSP server

For code intelligence, install an MCP server that exposes LSP operations and
add it to `mcp.toml` using the server's documented command and arguments:

```toml
[servers.lsp]
transport = "stdio"
command = "your-lsp-mcp-server"
args = ["--your-server-options"]
description = "Language-server code intelligence for this project"
capabilities = ["lsp", "definitions", "references", "diagnostics"]
request_timeout_secs = 300
```

Replace the command and arguments with those required by the MCP server;
AX does not launch a language server or translate LSP messages itself. The
MCP server decides which language servers to use. Discover its actual tool
names with `/mcp` or `mcp` `action="list_tools"` before calling them.
`request_timeout_secs` is a separate MCP request limit; increase it for
long-running language-server operations if needed.
Legacy skills that used `required_tools: [lsp]` should use
`metadata.ax.required-tools: "mcp"` after migration to `SKILL.md`
so they can route through the MCP gateway.

## Lazy connection & capability catalog

- Servers are **not** connected at startup. A connection (and process spawn)
  happens only when explicitly requested through `/mcp tools <server>` or when
  a proxy tool is invoked.
- `McpGateway` — a single `mcp` tool — exposes the capability catalog via
  `action="catalog"`: server name, description and declared capabilities, all
  without connecting. `action="list_tools"` / `action="call"` establish the
  real connection.
- `McpManager` records discovery and call failures, clears errors after
  successful operations, and caches discovered metadata. States (disabled /
  sleeping / connected / error) reflect actual configuration and runtime
  observations.

## Tool proxies

`McpToolProxy` converts a remote tool schema into an ordinary `Tool`, so the
agent loop needs no MCP branch. Dynamic names are `mcp__server__tool`, which
avoids cross-server collisions. Successful explicit discovery installs proxies
into the tool registry; disconnect/restart invalidates the agent runtime so
the next request gets updated tools.

## `/mcp` command

Inspect configured servers, their runtime status and discovered tools:

- Type to search server names, status or transport.
- Enter opens status, configured protocol, last error, and cached tool
  descriptions and schemas.
- **Alt+C** connects and discovers tools.
- **Alt+X** disconnects and clears cached tools/errors; future use can
  reconnect lazily.
- **Alt+R** disconnects, reconnects and rediscovers tools.
- Esc returns.

Connection failures are shown in the transcript and the manager while leaving
the UI open. `/mcp` does not edit configuration and does not implement MCP
OAuth. The inventory/status interaction is adapted from Codex without
importing its server framework.

## Reference

| Concern | Code |
|---|---|
| Config parsing | `crates/mcp/src/config.rs` |
| Client, transport boundary | `crates/mcp/src/client.rs`, `crates/mcp/src/transport/` (`stdio.rs`, `http.rs`, `websocket.rs`) |
| Manager, statuses, caching | `crates/mcp/src/manager.rs` |
| Gateway capability catalog | `crates/mcp/src/gateway.rs` |
| Bridge to tool registry | `crates/mcp/src/bridge.rs` |
| `/mcp` UI | `crates/cli/src/tui/commands/catalogs.rs` |
