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
powershell -ExecutionPolicy Bypass -c "irm https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1 | iex"
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
$env:AX_VERSION = "v0.1.0"; irm https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.ps1 | iex
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

### Slash commands

| Command | What it does |
|---|---|
| `/login` | Sign in with a provider |
| `/logout` | Remove a provider's credentials |
| `/model` | Browse and switch models |
| `/resume` | Pick a past session to continue |
| `/new` | Start a fresh session |
| `/memory` | Browse session, project and global memory |
| `/compact` | Compact the current context now |
| `/skills` | List and search installed skills |
| `/tools` | List available tools |
| `/mcp` | Manage MCP servers (connect / disconnect / restart) |
| `/permissions` | Review and change tool permission policy |
| `/status` | Show runtime, model, session and context info |
| `/exit` | Quit AX |

## Features

- **Persistent sessions** — every conversation is saved to a local SQLite
  database (`.ax/memory.sqlite3`); pick up where you left off with `/resume`.
- **Project & global memory** — `/memory` keeps long-term facts at session,
  project or global scope.
- **Context compaction** — when the context fills up, AX summarizes older
  messages automatically (at 75%) or on demand with `/compact`.
- **MCP** — connect Model Context Protocol servers over stdio, HTTP or
  WebSocket. Servers stay sleeping until you actually use their tools.
- **Skills** — load capability packs (a manifest plus instructions) by keyword,
  on demand.
- **Tools** — built-in `shell` and `filesystem` tools, plus MCP tools proxied
  through the same registry, guarded by a permission policy.
- **Multiple model providers** — DeepSeek, OpenAI, Codex and OpenAI-compatible
  endpoints side by side, switchable at any time.
- **Interactive permissions** — dangerous operations are denied by default,
  or ask for approval, depending on `/permissions`.

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
