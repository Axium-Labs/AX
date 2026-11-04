# Browser and Computer Use quickstart

## Computer Use

Computer Use defaults on unless explicitly disabled. Run `ax computer-use` to
inspect the current backend and configuration; `ax computer-use --enabled true`
re-enables it. Windows uses UIA, Linux AT-SPI and macOS Accessibility. Install the
platform dependencies and grant OS permissions described in
[Computer Use](computer-use.md#platform-dependencies-and-os-permissions).

Discover a window with `list_windows`, then read it with `read_window` before
using control IDs. Application access is separate from file/terminal confinement.
Saved application allows cover ordinary actions; explicit task Ask/Deny and
capability Deny remain effective. See [host permissions](host-permissions.md).

## Browser Use

The browser transport depends on the launch context:

- Local AXCrew desktop execution uses its sidebar WebViews through an authenticated
  bridge. Agent control currently requires Windows and the supported WebView2
  request guard. It reuses the embedded browser profile; screenshots and keys
  other than Enter/Tab are unavailable.
- Standalone AX uses Node and Playwright with AX-owned nonpersistent Chromium
  contexts. Install its runtime using the commands in
  [Browser Use](host-permissions.md#browser-use).
- Paired or SSH execution does not inherit the local desktop browser bridge.

Use CSS selectors for targets. For example:

```jsonl
{"action":"open","url":"https://example.com","session":"main"}
{"action":"snapshot","session":"main"}
{"action":"close","session":"main"}
```

Each HTTP(S) origin requires access. Cross-origin resources and redirects,
WebSockets, popups and downloads are restricted; third-party sign-in and CDN-heavy
pages may not work. Consult the canonical [browser contract](host-permissions.md#browser-use)
for operations, profile/privacy limits and transport-specific support. Browser
access does not change the file/terminal sandbox.
