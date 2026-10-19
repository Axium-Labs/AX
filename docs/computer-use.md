# Computer Use

Computer Use is a native host capability enabled by default. Windows uses UI
Automation, Linux uses AT-SPI, and macOS uses Accessibility. It reads application
windows and operates exposed controls. Linux requires a graphical desktop session;
WSL can operate Linux GUI applications but has no bridge to the Windows desktop.
SSH contexts do not expose local desktop tools. Desktop
access uses a separate authorized host transport, not the file/terminal worker;
it no longer requires selecting sandbox off. This does not add native Windows
OS confinement: explicit workspace/strict modes still fail closed there.
No driver starts at AX startup: an approved call starts a
disposable native worker with a 30-second timeout.

Unix workers have a 20-second total deadline. The host uses a private process
group to stop the helper and its Python driver together on cancellation/timeout.
Workers remove their private temporary code directory on normal completion or
their own timeout; abrupt host cancellation can leave inert temporary scripts. Drivers
are embedded production resources, not local test files. Windows retains its
native Rust worker and does not require Python.

## Platform dependencies and OS permissions

Linux needs Python 3, PyGObject and the AT-SPI typelib. On Ubuntu/Debian:

```sh
sudo apt-get install python3-gi gir1.2-atspi-2.0 python3-xlib python3-pil
```

AT-SPI must be enabled in the graphical session and the application must expose
accessibility metadata. X11 input/capture also requires XTest, XRes PID verification
and an EWMH window manager. Wayland currently supports accessibility reads,
control actions and EditableText; raw key/mouse input and capture are explicitly
unavailable, including the conservative XWayland path. No unscoped compositor or
desktop-wide capture fallback is used.

macOS needs Python 3 and PyObjC ApplicationServices, Quartz, Cocoa and
ScreenCaptureKit. Explicitly set `AX_HOME` to the intended AX/Crew home, then:

```sh
python3 -m venv "$AX_HOME/desktop/venv"
"$AX_HOME/desktop/venv/bin/python" -m pip install pyobjc-framework-ApplicationServices pyobjc-framework-Quartz pyobjc-framework-Cocoa pyobjc-framework-ScreenCaptureKit
```

Grant Accessibility and, for images, Screen Recording in System Settings →
Privacy & Security to the responsible launch context shown by macOS (terminal,
AXCrew or Python). API refusals remain errors; the driver does not bypass TCC or
automatically grant OS permissions. macOS 14+ uses target-window ScreenCaptureKit
screenshots; older systems use only the specified Quartz window.

Trusted `AX_DESKTOP_PYTHON` selects an interpreter. Otherwise Unix tries
`<AX_HOME>/desktop/venv/bin/python`, then `python3` on PATH. Isolated Python loads
the embedded adapters and installed dependencies; model input cannot select a
script, interpreter or arbitrary code. Missing dependencies produce an actionable
error and are never installed by a tool call.

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

Defaults: `enabled=true`, `include_screenshot=true`, `max_nodes=1200`,
`screenshot_width=1280`. Node counts accept 1–10000, longest screenshot edges
320–4096 pixels. Invalid edits fail before writing. Output contains `settings`,
platform/environment `supported`, `backend`, `requirements`, and `limits`.
`supported` identifies a native backend/session, not a dependency or TCC probe;
runtime availability is checked when called. Reads do not create a config.
Writes use AX's atomic save and retain other configuration, credentials and
session data. AXCrew saves switches after host confirmation; number fields save
on blur/Enter. Failed reads offer retry; failed writes retain confirmed values.

Missing `computer_use` sections or `enabled` fields use the enabled default.
An existing explicit `enabled=false` remains disabled; updating AX does not
overwrite that choice. Application access remains required. Existing application allows cover ordinary
computer actions without repeated generic approval; explicit task Ask/Deny and
capability Deny still apply. Minimal window discovery does not prompt. Unsupported platforms/environments still do not expose the desktop tool.

The runtime captures settings when building the turn registry. Off means the
model receives no desktop tool. Enabling/increasing limits applies on the next
registry build. Each helper also reloads host settings: disabling rejects later
calls from existing registries, and limits/screenshots use the stricter of the
captured and current values. Desktop actions cannot write settings or enable
access. This is a capability gate, not OS isolation from arbitrary programs
running as the same OS user; host shell permissions remain separate.

## Actions

- `list_windows`: visible top-level windows with opaque platform IDs, process IDs
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
  macOS additionally accepts `cmd`/`meta`; Linux X11 accepts `meta` (Super).
- `move_mouse`, `mouse_click`: foreground target, integer screen `x`/`y` inside
  its bounds. Hit-testing rejects covering windows. Buttons: left/right/middle.

All actions declare ComputerUse capability, RequiresApproval safety and exclusive
write access to `desktop:global`. Targeted actions also require independent app
access (once, session or always); generic shell/Process permission does not grant
it. They preserve the separate file/terminal boundary and action approvals.
See [host-permissions.md](host-permissions.md) for authorization and revocation.
Native accessibility objects stay inside disposable helper processes. Unix node
IDs combine hierarchy location with current native identity/role/name/bounds;
changed targets reject old IDs. Re-read after UI changes, especially on macOS
providers without stable accessibility identifiers.

## Privacy and limitations

Password nodes are redacted and their values are never read. Before screenshots,
an independent raw-tree scan checks up to 20000 nodes, including nodes omitted
by the user-visible read limit. Password fields, inaccessible providers,
incomplete scans and scan limits withhold the image. Protection covers password
controls exposed by application accessibility metadata; custom controls must
expose truthful metadata.

Windows PrintWindow, macOS target-window Quartz/ScreenCaptureKit and Linux X11
target-drawable capture never fall back to copying the desktop. Linux verifies
the X server's client PID against AT-SPI's D-Bus connection PID and rejects
overlapping windows before capture or coordinate effects. Unverifiable ownership,
nonstandard pixel formats, Wayland and unavailable providers withhold capture.
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
`desktop_unix.rs` preserves Rust configuration/app authorization and runs embedded
`desktop_unix/{worker,linux,macos}.py` lazily. No new unsafe Rust or arbitrary
Python/PowerShell evaluation is introduced.

Current native verification covers Windows fixtures and Linux X11 GTK fixtures,
including Unicode input, controls, focus, key/mouse events, image limits,
application denial/revocation, changed IDs and password fields beyond returned
tree limits. macOS privacy/key logic has isolated tests; actual Apple frameworks,
TCC, windows and captures still need a Mac acceptance run. The restricted Wayland
path is not compositor-accepted here. These limits do not establish a missing
native OS sandbox backend.
