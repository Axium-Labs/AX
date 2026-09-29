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

`ProviderSpec` identifies the vendor, its credential environment variable and
its `ProviderProtocol`. DeepSeek, Groq, Mistral, OpenRouter, Together, xAI and
Kimi use the shared `OpenAiCompatibleProvider` Chat Completions adapter.
`OpenAiCompatibleConfig::new` takes the selected model, key, endpoint and
context window; DeepSeek-specific endpoint and fallback defaults live in
provider metadata. OpenAI Responses and Codex retain their existing adapter.

| Provider | Endpoint | Credential |
|---|---|---|
| DeepSeek | Chat Completions | API key (`DEEPSEEK_API_KEY`) |
| OpenAI | Responses API | API key (`OPENAI_API_KEY`) |
| OpenAI Codex | ChatGPT Codex Responses | OAuth login (device flow) or legacy `--codex-auth` path |
| OpenAI-compatible vendors | Chat Completions endpoint from provider metadata | Provider API key (stored or environment variable) |

Adding a local model means implementing `ModelProvider` — no agent-loop
changes.

`ModelProvider::capabilities()` reports `vision` and `tool_calling`. OpenAI
Responses maps image content parts to `input_image`; AX enables this for known
vision-capable GPT model families (`gpt-4o`, `gpt-4.1`, `gpt-5`, `gpt-6`).
DeepSeek and generic OpenAI-compatible providers remain text-only until their
specific image wire format and model capability are established. Session
metadata preserves image parts separately from the plain-text transcript.

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
- `--provider` accepts a catalog provider id as well as the
  `deepseek` / `openai` / `codex` / `compatible` aliases, so vendors that
  appear several times can be addressed exactly — `xiaomi-token-plan-cn`,
  `xiaomi-token-plan-sgp` and `xiaomi-token-plan-ams` share model ids with
  `xiaomi` but not its endpoint. `compatible` alone is ambiguous once more
  than one OpenAI-compatible provider is credentialed.
- A bare `--model` that several providers list is narrowed to the ones with
  local credentials before it is resolved; if more than one remains, AX asks
  for `--provider` instead of guessing.

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
| Provider abstraction, providers | `crates/model/src/lib.rs`, `crates/model/src/providers.rs`, `openai_compatible.rs`, `openai.rs`, `codex_device.rs` |
| Auth storage | `crates/model/src/auth.rs` |
| Model catalog | `crates/model/src/registry.rs` |
| User config | `crates/cli/src/config.rs` |
| Provider detection, CLI resolution | `crates/cli/src/providers.rs`, `crates/cli/src/model_selection.rs` |
| `/login`, `/logout`, `/model` | `crates/cli/src/tui/commands.rs` |

## Catalog and connection status in Crew

AX ships an offline model catalog in `crates/model/src/catalog.json` (the existing
public pi catalog metadata, without endpoint overrides). An empty AX home no
longer needs a separately copied `pi-catalog.json`. Live discovery takes priority,
then a provider cache, then the installed or shipped bootstrap catalog. A failed
list request preserves its warning even when offline models remain selectable.
Offline entries do not prove authentication, balance or model entitlement.

Built-in provider endpoints take priority over model-cache endpoints. In
particular, old MiniMax Anthropic paths and Fireworks paths lacking `/v1` cannot
redirect the Chat Completions adapter. Provider, region and subscription remain
part of model identity; AX never silently sends a key to a different provider.

Crew reads the complete credential/support/model metadata from the `catalog`
field of `_ax/models`, instead of maintaining its own provider list or reading
provider caches. Older AX versions without this field produce an update message.
`_ax/refresh-models` returns `source` (`live`, `cache`, `fallback`) and `warning`.
Saving an API key discovers that provider; explicit refresh refreshes every
configured, supported provider, including providers with existing caches. Only
tool-capable models are offered. UI labels distinguish saved credentials and
local catalog models from successful online discovery; discovery is not an
inference test. Unsupported protocols remain explicitly disabled.

Endpoint references: [MiniMax OpenAI SDK](https://platform.minimax.io/docs/api-reference/text-openai-api)
and [Fireworks Chat Completions](https://docs.fireworks.ai/api-reference/post-chatcompletions).
