# Desktop platforms

AX uses one Rust workspace for Windows, Linux and macOS. CLI, TUI, ACP,
providers, sessions, memory and distributed workers share the same runtime.
Platform branches own shell processes, paths, credential permissions and
executable replacement; no second agent runtime is introduced.

## Build and install

Build natively with `cargo build --release --locked`. Windows uses `ax.exe`;
Linux/macOS use `ax`. The release matrix builds Windows x64, Linux x64/arm64
musl and macOS Intel/Apple Silicon. Unix archives contain executable `ax`,
`skills/` and `mcp.example.toml`, using the asset names expected by the installer
and updater. Platform check CI runs native build/lint/workspace tests on pull
requests or manual dispatch; release jobs remain tag-triggered. New assets are
only downloadable after a release containing them is actually published.

The Unix installer defaults to `~/.local/bin`, supports `AX_INSTALL_DIR` and
verifies `SHA256SUMS`. It stages beside the destination and renames atomically,
allowing updates while AX is running. Existing configuration, credentials,
sessions, user skills and MCP configuration survive. Set `AX_HOME` explicitly
to share state with Crew: Crew defaults to `~/.ax`, standalone AX defaults to
`<install-dir>/.ax`.

## Shell settings

`ax environment --terminal-shell default|sh|bash|zsh|fish` configures Crew's
Unix integrated terminal. Unix defaults to `default`, Windows to `powershell`.
`/environment` offers platform-appropriate choices; Windows retains PowerShell,
Command Prompt, Git Bash and WSL. Selecting a foreign shell is rejected. These
settings do not change the agent shell tool's POSIX sh/Windows PowerShell contract.

Crew resolves `default` from executable `$SHELL`, falling back to `/bin/zsh`
on macOS or `/bin/bash` on Linux, then `/bin/sh`. Explicit shells must be
installed. Imported Windows terminal preferences fall back to the Unix default
without rewriting saved config. WSL remains Windows-only.

## Capability limits

Native `desktop` adapters use Windows UIA, Linux AT-SPI and macOS Accessibility.
Unix requires its documented Python runtime; macOS also requires TCC permissions.
Linux X11 supports raw input/capture, while Wayland uses accessibility actions
only. macOS native desktop behavior still requires a Mac acceptance run. See
[computer-use.md](computer-use.md). NVIDIA NVML telemetry is optional;
unavailable GPU readings remain unavailable.

Workspace/strict OS confinement requires the existing Linux backend and
bubblewrap/prlimit dependencies. Native Windows/macOS confinement is still
unimplemented; explicit confined modes fail closed. See [security.md](security.md).
Platform adaptation does not relax sandbox or host-access decisions.

## Verification status

Current local verification covers Windows x64 and Ubuntu/WSL Linux x64 native
workspace tests and linting, plus a runnable Linux x64 musl binary. Native
Computer Use passed real Windows and isolated GTK/X11 fixture acceptance,
including independent application permission refusal and password protection.
This Linux run used a virtual X server, not a physical desktop or Wayland compositor.
macOS and
Linux arm64 build jobs are configured but have not been executed in this local
environment. Their native builds/runtime checks remain required before claiming
verified support or distributing those artifacts.
