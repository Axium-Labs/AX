# 0028 - Native desktop adapters

Date: 2026-10-14

## Decision

Keep one desktop tool schema, action-approval policy and independent application
permission store. Windows retains its Rust UIA driver. Unix uses embedded,
disposable Python adapters for Linux AT-SPI and macOS Accessibility/Quartz/
ScreenCaptureKit, behind the same Rust host transport.

The runtime remains lazy. It does not install Python dependencies or grant OS
privacy permissions. Native helper configuration and current app identity are
resolved before effects; persistent denial is checked again after Unix native
discovery. Temporary driver code is private. A dedicated process group stops
the helper and driver together on cancellation. Workers clean scripts on normal
completion/their own timeout; abrupt cancellation can leave inert private files.
The Unix operation deadline is shorter than the outer native worker deadline.
No arbitrary model-selected program or script is accepted.

## Boundaries

Graphical-session/platform support is separate from installed driver readiness
and macOS TCC grants. Linux raw input/capture requires verifiable X11 window
ownership and foreground/occlusion checks. Wayland currently uses accessibility
reads/actions only. Password/unreadable/incomplete UI scans withhold screenshots;
target-window capture never becomes a desktop screenshot.

These host adapters do not supply a missing native workspace sandbox. WSL can
operate Linux GUI apps, not Windows applications. SSH does not expose local UI.

## Validation

Windows and isolated Linux X11 fixture acceptance exercise actual UI APIs.
macOS privacy/key routing has isolated adapter tests, but native framework/TCC
acceptance and real Wayland compositor acceptance remain required.
