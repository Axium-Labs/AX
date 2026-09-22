# AX

A fast, lightweight AI agent for the terminal, built in Rust.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](Cargo.toml)
[![Rust 1.92+](https://img.shields.io/badge/rust-1.92+-orange)](https://www.rust-lang.org)

AX is a small, native terminal agent. It starts fast, streams responses, and
keeps sessions, memory, credentials and model config on your machine — no
service to sign up for, no telemetry.

- **Rust native** — one binary, no runtime, no Node, no Docker.
- **Fast startup** — providers, skills, MCP servers and memory load on demand.
- **Small binary** — a compact workspace of focused crates, not a framework.
- **Simple workflow** — type, get an answer, switch models, move on.
- **Local-first** — everything lives in `~/.ax` and your project's `.ax`.

No benchmarks are published yet; "fast" here means the startup path does as
little as possible by design.

## Quick Start

### Installing and running AX

Mac or Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.sh | sh
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "iex ((iwr 'https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1' -UseBasicParsing).Content)"
```

Then simply run:

```text
ax
```

Your first session:

```text
/login      # sign in with a provider (Codex OAuth or an API key)
/model      # pick a model
hello       # just start typing
```

That's it. Your session is saved automatically, and the model you pick is
remembered for next time.

## Installation

The installer fetches the latest release from GitHub, verifies the archive
against the release's `SHA256SUMS`, and installs a single `ax` / `ax.exe`
binary:

- **macOS / Linux** — installs to `~/.local/bin` by default and prints a
  PATH hint if that directory is not already on your PATH.
- **Windows** — installs to `%LOCALAPPDATA%\Programs\AX\bin` by default and
  adds it to your user PATH (only if it is not there already).

Both installers accept two environment variables:

| Variable | Purpose |
|---|---|
| `AX_VERSION` | Install a specific release tag instead of latest (e.g. `AX_VERSION=v0.1.0`) |
| `AX_INSTALL_DIR` | Override the install directory |

```bash
AX_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.sh | sh
AX_INSTALL_DIR="$HOME/bin" curl -fsSL https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.sh | sh
```

```powershell
powershell -ExecutionPolicy Bypass -c "$env:AX_VERSION='v0.1.0'; iex ((iwr 'https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1' -UseBasicParsing).Content)"
```

### Uninstall

Removing AX is just deleting the binary (and, on Windows, the PATH entry the
installer added). AX keeps its data separately in `~/.ax`, which you can delete
too if you want a clean slate.

**macOS / Linux**

```bash
rm -f ~/.local/bin/ax        # or wherever AX_INSTALL_DIR pointed
rm -rf ~/.ax                 # config, sessions, memory, model catalog (optional)
```

**Windows**

```powershell
Remove-Item -Force "$env:LOCALAPPDATA\Programs\AX\bin\ax.exe"
# remove the installer's PATH entry (optional but tidy)
$p = [Environment]::GetEnvironmentVariable('Path', 'User')
[Environment]::SetEnvironmentVariable('Path', ($p -split ';' | Where-Object { $_ -notlike '*Programs\AX\bin*' }) -join ';', 'User')
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\Programs\AX"   # leftover dir (optional)
```

## Development

### Build from source

Requires Rust 1.92+.

```bash
git clone https://github.com/Axium-Labs/AX.git
cd AX
cargo build --release
# binary at target/release/ax.exe (Windows) or target/release/ax (Unix)
```

## Providers & Models

AX discovers providers from what is already on your machine — credentials in
`~/.ax/auth.json` and standard environment variables like `DEEPSEEK_API_KEY`.
No network call happens during startup.

| Provider | Credential | How to use |
|---|---|---|
| OpenAI Codex | OAuth login | `/login` → Codex OAuth |
| DeepSeek | API key | `/login` → API key, or `DEEPSEEK_API_KEY` |
| OpenAI | API key | `/login` → API key, or `OPENAI_API_KEY` |
| OpenAI-compatible | API key + base URL | `OPENAI_API_KEY` / `OPENAI_BASE_URL` |

Models are discovered dynamically into `~/.ax/models/` (a bundled catalog plus
a per-provider refresh cache). Switch anytime with `/model`; the last
selection — provider, model and reasoning effort — is persisted to
`~/.ax/config.toml` and restored on the next launch.

To pick a provider explicitly from the command line:

```bash
ax --provider deepseek
ax --provider openai-codex --model <model-id>
ax run "explain this repo" --provider deepseek
```

## Usage

Start the UI (default):

```bash
ax
```

Run one task and exit (non-interactive):

```bash
ax run "summarize the TODOs in ./src"
```

Run independent tasks with bounded concurrency:

```bash
ax agents "review pr #12" "write changelog" --concurrency 2
```

## Design

**Fast. Lightweight. Simple.**

AX deliberately does not load everything up front. Skills, MCP servers,
providers and memory are all activated only when the task actually needs them —
so the things that make an agent powerful never get in the way of starting it
or of a normal conversation. The trade-off is intentional: capability is loaded
on demand, not at startup.

## Advanced

The internal architecture — agent loop, context management, multi-agent
scheduling, SQLite schema, module layout — is documented in
[docs/architecture.md](docs/architecture.md).

## License

MIT OR Apache-2.0.
