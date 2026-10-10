# Computer Use

Computer Use is an opt-in native Windows host capability. It reads application
windows through Windows UI Automation and operates exposed controls. macOS,
Linux, WSL execution and SSH contexts do not expose the desktop tool. Desktop
access uses a separate authorized host transport, not the file/terminal worker;
it no longer requires selecting sandbox off. This does not add native Windows
OS confinement: explicit workspace/strict modes still fail closed there.
No driver starts at AX startup: an approved call starts a
disposable native worker with a 30-second timeout.

## Configuration

AX owns `computer_use` in `AX_HOME/config.json` (installation `.ax` by default).
AXCrew's Computer Use page uses the typed local `ax_computer_use` Tauri command
and the current AX CLI, without a workspace, provider or active session.
Crew supplies its existing global `AX_HOME` (explicit environment override,
otherwise `~/.ax`), shared with its local chat runtime. Standalone AX retains
its installation-home default unless `AX_HOME` is set.

```sh
ax computer-use
ax computer-use --enabled true
ax computer-use --include-screenshot false --max-nodes 1200 --screenshot-width 1280
ax computer-use --enabled false
```

Defaults: `enabled=false`, `include_screenshot=true`, `max_nodes=1200`,
`screenshot_width=1280`. Node counts accept 1–10000, longest screenshot edges
320–4096 pixels. Invalid edits fail before writing. Output contains `settings`,
platform/environment `supported`, and `limits`. Reads do not create a config.
Writes use AX's atomic save and retain other configuration, credentials and
session data. AXCrew saves switches after host confirmation; number fields save
on blur/Enter. Failed reads offer retry; failed writes retain confirmed values.

The runtime captures settings when building the turn registry. Off means the
model receives no desktop tool. Enabling/increasing limits applies on the next
registry build. Each helper also reloads host settings: disabling rejects later
calls from existing registries, and limits/screenshots use the stricter of the
captured and current values. Desktop actions cannot write settings or enable
access. This is a capability gate, not OS isolation from arbitrary programs
running as the same Windows user; host shell permissions remain separate.

## Actions

- `list_windows`: visible top-level windows with decimal handle IDs, process IDs
  and verified app executable identities; denied applications are hidden and
  titles are withheld until persistent app access is allowed. Optional
  `include_hidden` includes offscreen windows.
- `read_window`: requires `window_id`; flat `ui_tree.nodes` contain IDs,
  parent IDs, roles, names, optional values, bounds, focus and enabled state.
  Optional `max_nodes` can only lower the configured limit. `truncated` reports
  omission; a hard depth limit also prevents pathological traversal.
- `screenshot_window`: target-window PNG subject to settings and privacy checks.
  Whole-screen capture is unavailable.
- `focus_window`: explicit focus change; refusal is reported.
- `click_element`: uses Invoke, Toggle, SelectionItem or ExpandCollapse patterns.
  Requires `window_id` and an `element_id` from a fresh read.
- `type_text`: `window_id`, `text`, optional `element_id`; otherwise uses the
  focused control only if it belongs to the target. Editable ValuePatterns are
  set directly; fallback Unicode input requires foreground focus.
- `press_key`: `window_id`, named `key`, optional `ctrl`/`shift`/`alt` modifiers.
  Supports navigation keys, F1–F12, letters and digits. Modifiers release on
  completion and normal errors.
- `move_mouse`, `mouse_click`: foreground target, integer screen `x`/`y` inside
  its bounds. Hit-testing rejects covering windows. Buttons: left/right/middle.

All actions declare ComputerUse capability, RequiresApproval safety and exclusive
write access to `desktop:global`. Targeted actions also require independent app
access (once, session or always); generic shell/Process permission does not grant
it. They preserve the separate file/terminal boundary and action approvals.
See [host-permissions.md](host-permissions.md) for authorization and revocation.
Native COM objects stay inside the disposable helper process.

## Privacy and limitations

Password nodes are redacted and their values are never read. Before screenshots,
an independent raw-tree scan checks up to 20000 nodes, including nodes omitted
by the user-visible read limit. Password fields, inaccessible providers,
incomplete scans and scan limits withhold the image. Protection covers password
controls exposed by application accessibility metadata; custom controls must
expose truthful metadata.

PrintWindow captures only the target window, with no desktop-copy fallback.
Images preserve aspect ratio, do not enlarge small windows, and cap the longest
edge. PNG paths refer to OS temporary storage; the host transport returns inline
image output without opening host filesystem access. Some GPU renderers, elevated
applications and minimized windows cannot be captured/operated. Capture failure
returns `screenshot_skipped` and leaves the UI tree usable; AX does not bypass
OS protected/elevated desktop access. Screenshots are not automatically deleted
because the agent may read them later.

## Modules and validation

CLI `computer_use.rs` handles settings; runtime/builder.rs gates registration.
Tool `desktop.rs` owns validation, permissions and helper lifetime;
`desktop_windows.rs` uses safe uiautomation, enigo, win-screenshot and image APIs.
No new unsafe Rust, external driver, Python or PowerShell runtime is required.
Local `test/` regressions cover settings, invalid writes, revocation, actual UIA
reads/typing/invocation, focus, mouse, keyboard, truncation, captures and password
fields beyond the returned node limit, using only disposable fixture windows.
