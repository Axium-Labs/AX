# 0024 — Separate workspace confinement from host UI access

Status: accepted, 2026-10-10.

Previously desktop use required sandbox off and used generic Process permission. The browser relied on an external CLI whose sessions and redirect behavior could not reliably enforce AX website grants. Workspace access, application/site access and approval of a particular operation must be independent decisions.

Add `ExecutionBoundary::AuthorizedHost` plus typed host-access requests/authorizations to the existing Tool and ApprovalPolicy contracts. Keep WorkspaceWorker/SandboxManager for files, commands, Skills and local MCP. Resolve application identity from the target process executable and websites by exact HTTP(S) origin; require once/session/persistent authorization even when ordinary capability/profile approvals allow an action. Unknown approval implementations deny host access by default.

Store persistent rules in AX home, independently of config and workspace data. Scope ephemeral access and browser sessions to the existing controller PermissionStore. Recheck identity and revocation before execution. CLI, TUI, ACP and Mod builtin calls share this boundary; AXCrew is a settings/approval frontend, not an alternative authority.

Own the browser through an embedded, fixed Node/Playwright worker rather than a model-invocable external CLI. Block unapproved origins, every redirect hop and uncontrolled browser channels; do not import a personal profile. Return screenshots through existing image output rather than widening filesystem permissions.

This does not add Windows/macOS OS sandbox backends or non-Windows desktop drivers. Current browser policy blocks cross-origin resources and WebSockets and therefore limits site compatibility. Native extensions and unrestricted same-user programs remain trusted. Detailed current behavior and limitations are in [host-permissions.md](../host-permissions.md).
