# Development

How to build, test and extend AX. For how the pieces fit together, start with
[architecture.md](architecture.md); for the documentation map, see
[docs/README.md](README.md).

## Prerequisites

- Rust **1.92+** (workspace `rust-version`, edition 2024)
- No runtime, Node or Docker required to *run* AX; Linux musl and macOS
  release builds use `zig` / `cargo-zigbuild` via CI (see
  [Releasing](#releasing)).

## Workspace layout

```text
Cargo.toml               # workspace root: 9 crates, shared deps, lints, release profile
crates/
├── cli/                 # composition root: args, provider selection, TUI, sessions
├── core/                # runtime-core: agent loop, context, budget, compression
├── evolution/           # low-frequency learning of owned Skills and Project facts
├── lexical/             # shared language-independent lexical features + similarity
├── memory/              # SQLite + JSONL storage, scoped facts, resume
├── mcp/                 # MCP client, transports, gateway, proxies
├── model/               # provider abstraction, auth storage, catalogs
├── skill/               # skill metadata indexing and routing
└── tool/                # Tool trait, built-ins, permissions, telemetry
```

## Build & test

```bash
cargo build --release        # binary at target/release/ax(.exe)
cargo test --workspace       # all unit tests
cargo test --package memory  # one crate
cargo clippy --workspace --all-targets   # lint (all + pedantic warnings)
cargo fmt --all              # formatting
```

Quick smoke test of a local build:

```bash
./target/release/ax --version
./target/release/ax run "hello" --provider <configured-provider>
```

## Conventions

Web scheduling benchmarks and the optional public-provider smoke test are
documented in [web-latency.md](web-latency.md).

- **One-directional dependencies**: `cli` composes; core crates do not depend
  on terminal UI. Keep new modules on the same side of the boundary.
- **Lazy by default**: providers, skills, MCP servers and memory load on
  demand. Don't move work into the startup path.
- **Permissions are declared, not guessed**: every tool returns
  `ToolPermission { capability, safety }`; never infer permissions from tool
  name strings.
- **Context space derives from `ContextBudget`**: no new magic character
  counts or fixed fractions of the window. See [context.md](context.md).
- **Raw history is never deleted**: compression only changes what is fed to
  the model; storage changes must keep the JSONL event stream authoritative.
- **Lints**: `unsafe_code` is forbidden; clippy `all` + `pedantic` warn.

## How to add

### A new model

1. For a new protocol, implement `ModelProvider` in `crates/model` (see
   `openai_compatible.rs` / `openai.rs` for shape). Providers that use the
   existing Chat Completions protocol reuse `OpenAiCompatibleProvider`.
2. Register the provider id/endpoint/env-var and protocol in the provider catalog.
3. Wire resolution in `crates/cli/src/model_selection.rs` and the login
   options in the TUI.

### A new built-in tool

1. Implement `Tool` in `crates/tool` (name, description, JSON input schema,
   `permission()`, and explicit read/write resources through `resources()`).
   Unknown effects use a global exclusive resource lease; declaration is
   required to gain safe parallelism. See [tools.md](tools.md#concurrency).
2. If the tool supports isolated child runs, implement `fork_for_run` and bind
   all workspace/session/memory/process state to the supplied `RunContext`.
   Otherwise it is omitted from child registries.
3. Register it in the composition root's registry (`cli::tools` in
   `crates/cli/src/main.rs`).

### A new MCP transport

1. Implement the transport request boundary in
   `crates/mcp/src/transport/`.
2. Add the transport type to config parsing and the client builder.

### A new skill

1. Create `skills/<name>/SKILL.md` with standard YAML frontmatter
   (`name`, `description`, optional `license`, `compatibility`, `metadata`,
   and `allowed-tools`) and a Markdown body. Describe both the task and its
   use cases in `description`, in the words and language your users will type:
   routing is lexical similarity, so a description that shares no vocabulary
   with a request only reaches the model through the metadata catalog. See
   [skills.md](skills.md) for validation and precedence. Legacy `skill.toml`
   plus `instructions.md` only supports existing user data.
2. No code changes needed — indexing and routing are automatic.

### A new UI

1. Consume `AgentEvent` and call the kernel; the existing ratatui TUI in
   `crates/cli/src/tui/` is the reference.

### Storage changes

1. Keep session/message repository semantics in `crates/memory`.
2. Add **additive** migrations guarded by `PRAGMA user_version`; never drop
   raw events. See [storage.md](storage.md).

## Releasing

Releases are driven by git tags: pushing a `v*` tag to GitHub triggers the
`Release` workflow (`.github/workflows/release.yml`).

```bash
git tag -a v0.2.0 -m "AX 0.2.0"
git push origin v0.2.0
```

The workflow:

1. Builds the Windows x64 release binary.
2. Packages `ax-x86_64-pc-windows-msvc.zip` with the executable, `skills/`
   and `mcp.example.toml`; generates `SHA256SUMS` and publishes the GitHub Release.
   The release workflow does not run tests.

Installers (`scripts/install.sh`, `scripts/install.ps1`) fetch the latest
release from GitHub and verify the archive against `SHA256SUMS`, then place the
bundled `skills/` in `$AX_HOME/skills` (skipping packages that already exist)
and copy `mcp.example.toml` to `$AX_HOME/mcp.toml` only when no config exists.
The explicit `ax --update` command uses the same release and checksum. It
updates the running executable's path and refreshes that same bundled payload;
on Windows a detached helper waits for AX to exit before replacing the verified
binary. Update code lives in
`crates/cli/src/update.rs` and does no work on ordinary startup. User and
project data directories are otherwise untouched.
