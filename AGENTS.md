# AGENTS.md

This file is the entry point for Codex (or any AI agent) working in this
repository. Read it first, then follow the links to the topic documentation
you need.

## What is this repo

AX is a fast, lightweight AI agent for the terminal, written in Rust: a single
native binary with sessions, memory, credentials and model config stored
locally. It is an **agent runtime kernel** — the kernel handles context,
models, tools, events, permissions and the execution loop; skills and the
caller decide task behavior.

## Repository layout

```text
README.md                # user-facing overview, install, usage
AGENTS.md                # this file — Codex entry point
docs/
├── README.md            # documentation map / routing table
├── architecture.md      # system architecture (start here)
├── memory.md            # memory scopes, identity, persistence
├── context.md           # context budget, compression, resume
├── storage.md           # SQLite schema, JSONL events, migration
├── tools.md             # tools, permissions, safety
├── mcp.md               # MCP configuration and integration
├── skills.md            # skill package format and routing
├── providers.md         # models, providers, auth
├── development.md       # build, test, extend, release
└── adr/                 # architecture decision records
crates/
├── cli/                 # composition root (args, TUI, sessions, wiring)
├── core/                # runtime-core (agent loop, context, budget)
├── memory/              # SQLite + JSONL storage, scoped facts, resume
├── mcp/                 # MCP client, transports, gateway
├── model/               # provider abstraction, auth, catalogs
├── skill/               # skill metadata indexing and routing
└── tool/                # Tool trait, built-ins, permissions, telemetry
scripts/                 # install.sh / install.ps1
.github/workflows/       # release.yml (tag-driven release pipeline)
```

## Start here

1. **[docs/README.md](docs/README.md)** — the documentation map; pick the
   topic for your task.
2. **[docs/architecture.md](docs/architecture.md)** — required reading before
   changing code: crate responsibilities, the agent loop, and the cold-start
   guarantees.
3. **[docs/development.md](docs/development.md)** — build/test commands,
   conventions, and "how to add a model/tool/skill/…" walkthroughs.

## Commands

```bash
cargo build --release        # binary at target/release/ax(.exe)
cargo test --workspace       # all unit tests
cargo clippy --workspace --all-targets
cargo fmt --all
```

Smoke test a local build: `./target/release/ax --version` (or `ax.exe`).

## Non-negotiable rules

- **Dependencies stay one-directional**: `cli` composes everything; core
  crates never depend on terminal UI.
- **Lazy by default**: providers, skills, MCP servers and memory load on
  demand. Never add work to the startup path.
- **Permissions are declared, not guessed**: every tool returns
  `ToolPermission { capability, safety }`; never infer permissions from tool
  name strings.
- **Context limits derive from `ContextBudget`**: no new magic character
  counts or fixed fractions of the context window.
- **Raw history is never deleted**: compression only changes what is fed to
  the model; storage changes must keep the JSONL event stream authoritative.
- **Lints**: `unsafe_code` is forbidden; clippy `all` + `pedantic` warn.
- **Docs stay in sync**: if a change affects behavior, update the matching
  topic document; record lasting decisions as ADRs
  ([docs/adr/](docs/adr/README.md)).

## Common tasks

| Task | Follow |
|---|---|
| Add a model provider | [docs/providers.md](docs/providers.md), [docs/development.md](docs/development.md#a-new-model) |
| Add a built-in tool | [docs/tools.md](docs/tools.md), [docs/development.md](docs/development.md#a-new-built-in-tool) |
| Add an MCP transport | [docs/mcp.md](docs/mcp.md), [docs/development.md](docs/development.md#a-new-mcp-transport) |
| Add a skill | [docs/skills.md](docs/skills.md), [docs/development.md](docs/development.md#a-new-skill) |
| Change storage/schema | [docs/storage.md](docs/storage.md) (additive migrations only) |
| Change context/compression | [docs/context.md](docs/context.md) |
| Release a version | tag `v*` → CI; see [docs/development.md](docs/development.md#releasing) |

## Before you finish

- Run `cargo test --workspace` and `cargo clippy --workspace --all-targets`.
- Re-read [docs/architecture.md](docs/architecture.md) rules if you touched
  the kernel or storage.
- Update the documentation map if you added or renamed a document.
