# Models, Providers & Auth

This document covers provider abstraction, credentials, model discovery and
selection in AX.

## Overview

AX discovers providers from what is already on your machine — credentials in
`~/.ax/auth.json` and standard environment variables like `DEEPSEEK_API_KEY`.
**No network call happens during startup.** The provider is built only on the
first model call.

`cli::providers` is the single definition of "which providers are
configured": AX's own `auth.json`, conventional environment variables, and an
explicit legacy Codex auth path. Both CLI startup resolution and the TUI's
`/model` catalog refresh delegate to it.

## Provider abstraction

The `model` crate defines the provider-neutral `ModelProvider` trait, message
types, tool calls and responses. `complete_stream` is the unified streaming
entry point, with a non-streaming fallback by default. Providers also report
their context window, which drives the kernel's compression policy.

| Provider | Endpoint | Credential |
|---|---|---|
| DeepSeek | Chat Completions | API key (`DEEPSEEK_API_KEY`) |
| OpenAI | Responses API | API key (`OPENAI_API_KEY`) |
| OpenAI Codex | ChatGPT Codex Responses | OAuth login (device flow) or legacy `--codex-auth` path |
| OpenAI-compatible | Custom base URL | API key + `OPENAI_BASE_URL` |

Adding a local model means implementing `ModelProvider` — no agent-loop
changes.

## Credentials

Credentials live in `~/.ax/auth.json` (or `$AX_HOME/auth.json` when set):

```json
{
  "deepseek": { "type": "api_key", "key": "sk-..." },
  "openai-codex": {
    "type": "oauth",
    "access": "eyJ...",
    "refresh": "...",
    "expires": 1234567890,
    "account_id": "org-..."
  }
}
```

- **Permissions**: on Unix the file is written with mode `0600`; on Windows AX
  calls `icacls` to remove inherited ACEs and grant the current user full
  control (best effort — credential saving never depends on the tool
  succeeding).
- **Resolution order** (`AuthStorage::resolve_api_key`): AX's own stored
  credential, then the conventional environment variable. OAuth providers use
  `resolve_oauth`.
- Older AX builds stored credentials in the project's data directory;
  `migrate_legacy_project_auth` copies that once into `~/.ax/auth.json`. AX
  never imports another application's credentials (notably
  `~/.codex/auth.json`) unless an explicit `--codex-auth` path is given.
- `~/.ax/auth.json` is written atomically (temp file + rename).

## Model discovery

Models are discovered dynamically into `~/.ax/models/`: a bundled catalog
plus a per-provider refresh cache (`models/<provider>.json`). The cache avoids
API requests on every startup; refresh falls back to the cache after a 15s
timeout.

## Model selection

- **`/model`** — choose a configured provider model and, where supported, its
  reasoning mode. Accepts an optional reference, e.g. `/model provider/model-id`.
  An exact match against the cached snapshot is applied directly; otherwise the
  picker opens with the argument as search text and refreshes configured
  provider catalogs in the background. Only models from configured providers
  are offered.
- The last selection — provider, model and reasoning effort — is persisted to
  `~/.ax/config.json` (`AxConfig { model: { provider, model, reasoning_effort } }`)
  and restored on the next launch. A legacy `config.toml` is migrated
  automatically.
- CLI overrides (`--provider`, `--model`, `--codex-auth`,
  `--context-window`) take precedence over the persisted selection, which
  itself precedes local provider detection.

## Auth flows

- **`/login`** — connect a provider. API key providers open `SecretInput`; the
  submitted key is stored through `AuthStorage`. Codex OAuth starts a
  background device-code flow; the UI receives the verification prompt and
  completion status. After saving an API key, AX invalidates the runtime and
  refreshes model catalogs asynchronously. `/login` does not select a model —
  use `/model` afterwards.
- **`/logout`** — remove the credential AX stores for a selected provider and
  invalidate the runtime. It does not modify environment variables or
  credentials managed by a provider's own CLI.
- **Codex OAuth refresh**: `refresh_codex_credential_if_needed` refreshes the
  token before it expires (60s margin) when a Codex session is about to start.

## Reference

| Concern | Code |
|---|---|
| Provider abstraction, providers | `crates/model/src/lib.rs`, `crates/model/src/providers.rs`, `deepseek.rs`, `openai.rs`, `codex_device.rs` |
| Auth storage | `crates/model/src/auth.rs` |
| Model catalog | `crates/model/src/registry.rs` |
| User config | `crates/cli/src/config.rs` |
| Provider detection, CLI resolution | `crates/cli/src/providers.rs`, `crates/cli/src/model_selection.rs` |
| `/login`, `/logout`, `/model` | `crates/cli/src/tui/commands.rs` |
