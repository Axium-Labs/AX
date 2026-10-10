# Host access, sandbox and action approvals

AX separates three decisions:

- File/terminal confinement: existing `SandboxManager` and `WorkspaceWorker` tools enforce the configured workspace boundary. Host access never changes this mode or retries a rejected command outside it.
- Host access: `AuthorizedHost` tools resolve the real application executable or exact HTTP(S) origin and require independent access. Process, shell, Network and `--allow-dangerous` grants do not grant unknown applications/websites.
- Actions: existing capability rules, task permission ceilings and approval policies still apply to each tool call. Access to an application/site does not approve every action in it.

The scheduler resolves access after ordinary action approval, obtains a typed authorization and passes it through `execute_output_authorized`. Before executing, the tool resolves its target again and rechecks revocation. Mods calling built-ins use the same gate. Workspace wrappers forward the authorization contract. No model-provided input field creates access.

## Grants and settings

Unknown targets default to `ask`; choices are once, this session, always, or deny. Once covers the current invocation and is not saved. Session grants belong to one controller permission scope and AX home; another ACP/chat session cannot reuse them. Session reset, resume/new-session boundaries and process exit expire them. Persistent grants and denials are stored in `AX_HOME/host-permissions.json` separately from `config.json`. Explicit deny and the browser off switch win over previously obtained approvals. Removing a saved rule returns the target to ask and clears its session grants.

The file contains `browser_enabled` (default true), `apps` and `sites`, each map using `allow`, `ask` or `deny`, plus opaque `versions` metadata. Per-target epochs make revocation/removal from a separate UI/CLI process invalidate old session grants without resetting unrelated targets. Switching Browser Use off/on also invalidates its ephemeral grants. Application identities are canonical executable paths (case-normalized on Windows), resolved from the actual window process. Website identities contain scheme, host and port, not a wildcard domain or page path. HTTP and HTTPS, subdomains and nondefault ports are distinct.

```sh
ax host-permissions
ax host-permissions --surface computer --target "C:/Program Files/Example/app.exe" --decision ask
ax host-permissions --surface browser --target https://example.com --decision allow
ax host-permissions --surface browser --target https://example.com --decision deny
ax host-permissions --surface browser --target https://example.com --remove
ax host-permissions --browser-enabled false
```

The early CLI path works without a workspace/model/session. Reads do not create files. Invalid edits fail before writing. Writes use an interprocess lock and atomic replacement, retaining unrelated application/site entries, AX configuration and credentials. Corrupt permissions fail closed. AXCrew edits these same rules through its typed `ax_host_permissions` bridge and shows only confirmed values; its ACP permission dialog supports the four choices.

## Computer Use

The Windows desktop worker is a host transport, separately authorized from confined file/terminal workers. Computer Use still defaults off and only the user settings path enables it. `list_windows` exposes minimal app/handle metadata for discovery, hides denied apps, and withholds titles for apps without a persistent allow. Targeted reads, screenshots and effects require app access. No arbitrary desktop-wide screenshot or shell command is added. Images use the existing multimodal `ToolOutput::Image` channel; their temporary storage does not grant the agent arbitrary host-file access. See [computer-use.md](computer-use.md).

## Browser Use

`browser.rs` starts the embedded `browser_worker.mjs` lazily, through Node and Playwright, outside the file/terminal worker. It creates AX-owned, nonpersistent Chromium contexts with the Chromium sandbox enabled and no personal browser profile. Contexts are shared across turns only within the same controller permission scope. Closing the final session closes Chromium; dropping a scope closes the driver's input pipe, allowing normal browser cleanup. A worker deadline closes owned sessions; the Rust exchange has a separate timeout.

Install the external runtime explicitly (AX does not run package installation from a browser tool call):

```sh
npm install --prefix "<AX_HOME>/browser" playwright
npx --prefix "<AX_HOME>/browser" playwright install chromium
```

Node must be available. Trusted host environment overrides are `AX_BROWSER_NODE`, `AX_BROWSER_PLAYWRIGHT`, `AX_BROWSER_CHANNEL` (e.g. `msedge`) and `AX_BROWSER_HEADLESS=true`. They are not model tool arguments.

Actions: `open`, `goto`, `snapshot`, `screenshot`, `click`, `fill`, `type`, `press`, `reload`, `go_back`, `go_forward`, `list_sessions`, `close`. `target` is a CSS selector, not a Playwright CLI element reference. Session names accept 1–64 ASCII letters/digits/hyphens/underscores. URL navigation requires HTTP(S) without embedded credentials. Browser Use has its own capability; read snapshots/screenshots are safe actions but still require website access. Domain profile denials and the task's network ceiling remain effective at the transport.

Current policy is deliberately limited to the active authorized origin. Cross-origin resources, navigations, iframe requests and redirect hops are blocked before their request is sent; to visit another site, issue an explicit `goto` and authorize its origin. Service workers, WebSockets, popups, downloads, uploads and arbitrary model-supplied JavaScript are unavailable. This can break sites depending on CDNs, third-party sign-in or WebSockets. Manual redirect fetching preserves the requested page URL rather than exposing the final same-origin redirect URL, which can affect relative links. Page content is untrusted; these are current functionality limits, not promises of full browser compatibility.

Accessibility snapshots have a 32000-character transport cap and titles/URLs are bounded. Password-bearing pages withhold snapshots and screenshots. Password/custom-widget protection depends on truthful accessibility/HTML metadata. Screenshots return inline PNGs; no personal cookies or login profile are imported. Request error call logs are stripped so Cookie/Authorization headers are not returned. Cancelled exchanges close their pipes and cannot leave a queued response for a later call.

## Platform and trust limits

Permission separation does not create a missing OS backend. Real native workspace/strict isolation currently exists only on Linux; explicit confined modes on native Windows/macOS still fail closed, and the platform default remains off there. Native Computer Use is still Windows-only; WSL/Linux/macOS desktop drivers and an IPC bridge from Linux AX to a Windows desktop broker are not implemented. SSH contexts do not expose local host UI tools. Browser use needs a supported Node/Playwright/browser installation and was live-tested here on Windows Edge.

These settings govern AX's trusted tool entry points, not arbitrary programs already running as the same OS user. Unrestricted shell access or controlling a general-purpose terminal/browser application can perform broader actions. Keep the existing OS sandbox wherever supported; application/site permission checks are a separate service boundary, not a substitute for OS confinement. Local native AX extensions remain part of the trusted computing base.

See [ADR 0024](adr/0024-independent-host-access.md) for the architectural decision.
