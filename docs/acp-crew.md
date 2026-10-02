# ACP and AX Crew integration

`ax acp` is an additive [Agent Client Protocol v1](https://agentclientprotocol.com/protocol/v1/initialization) stdio adapter in `crates/cli/src/acp.rs`. It uses the existing CLI composition root: `ReplState` creates or restores sessions and `run_prompt_with` invokes `AgentKernel::run_turn_checkpointed`. It does not implement a second model loop, tool registry, memory store, or credential store.


CLI/TUI/ACP all bind the same LocalChildHost and child budget through
`child_runtime::configure_controller`. ACP's `session/prompt` invokes
`run_session_prompt`, which uses the shared prompt runner. A model-created queue
with `execution="children"` automatically enters the kernel's
`execute_next_child()` for each running task, executes isolated children
sequentially and emits only the controller summary. Child failures advance
independent tasks. `--child-timeout-secs` applies independently of the controller
turn timeout in Crew/ACP too. Child tool updates retain session-prefixed call IDs.

Prompt lists never imply executable tasks. After reading a dataset, the model can
call `task_queue start` with actual instances and complete `{title,input}` task
objects. A mistaken queue with no dispatched work can be explicitly replanned;
its previous state remains archived in authoritative history.

The adapter accepts `initialize`, `session/new`, `session/load`, `session/resume`, `session/prompt`, and `session/cancel`. JSON-RPC messages are one JSON object per line. The ACP session ID is the AX `MemoryStore` session UUID. `session/load` replays stored user, assistant, and tool messages as `session/update` notifications; `session/resume` restores context without replay. ACP clients must send a `cwd` equal to the ACP process working directory. Crew starts one `ax acp` process in the member's configured directory, which also keeps AX's existing project-root and tool-workspace resolution intact.

`AgentEvent` deltas and tool start/finish events become `session/update` notifications. Lifecycle updates use the model's original `tool_call_id`, so parallel calls to the same tool remain distinct. The adapter's `ApprovalPolicy` maps `session/request_permission` responses into the existing `PermissionStore`. Explicit Deny remains authoritative. `session/cancel` aborts the current turn future and returns `stopReason: cancelled`; it does not promise to undo a tool's external effects. The next resume uses AX's existing interrupted-tool recovery and never replays a tool automatically.

ACP `mcpServers` stdio and HTTP entries are converted in memory to AX `McpConfig` and passed to the existing lazy `McpManager`. No MCP credentials are written to Crew's database. ACP prompt text and resource links are supported. Other content blocks return an error instead of being silently dropped. AX-specific read-only methods include `_ax/status`, `_ax/models`, `_ax/capabilities`, `_ax/skills` (the project and global skill catalogs, with any missing required tools per skill), `_ax/mcp` (configured servers with description, enabled flag and declared capabilities), and `_ax/tools` (AX's built-in tool catalog). `_ax/tools` lists only the tools AX ships: MCP-provided tools exist per session only after connecting to a server, so `_ax/mcp` reports the servers instead.

Crew supplies optional `_ax` session metadata. `skills` is an allowlist of locally installed AX skill names, `mcpServers` is an allowlist of locally configured AX MCP server names, and `permissionProfile` is `ask`, `allow`, or `deny`. Crew stores names and policy, never MCP environment values or provider credentials. The adapter applies these to `ReplState` and `PermissionStore` without changing the kernel. ACP clients that omit `_ax` use normal AX skill and MCP behavior.

`ax crew pair <code> --gateway <https-url>` generates or loads a device Ed25519 private key under AX home, sends only the public key to Crew, and records the device ID after a successful one-time redemption. `ax crew connect <https-url>` initiates an outbound WSS connection, signs the gateway's fresh challenge, sends heartbeats, reconnects with backoff, and runs routed work through child `ax acp` processes. HTTP/WS is accepted only for loopback development. TLS termination for WSS is deployed in front of Crew; the AX binary verifies the gateway certificate.

The corresponding Crew backend and its API are in the sibling `ax_crew` project. Crew stores orchestration metadata and final task outputs, while AX keeps message history, context snapshots, memory, tool results, and provider credentials. Live Crew WebSocket events can include message deltas and permission requests; its SQLite event index stores only redacted event metadata.

## Workspace identity and confinement

Device run requests must carry workspace_id. AX resolves that ID through its local
registered-project list and canonicalizes the result. Supplied cwd is rejected;
older gateways must migrate to this contract. Device ACP children use strict sandbox
mode and cannot switch away from their bound root through ACP requests. See
[security.md](security.md).
