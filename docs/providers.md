# Models, Providers & Auth

This document covers provider abstraction, credentials, model discovery and
selection in AX.

## Overview

AX discovers providers from what is already on your machine — credentials in
`<install-dir>/.ax/auth.json` and standard environment variables like `DEEPSEEK_API_KEY`.
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

Credentials live in `<install-dir>/.ax/auth.json` (or `$AX_HOME/auth.json` when set):

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
  `migrate_legacy_project_auth` copies that once into `<install-dir>/.ax/auth.json`. AX
  never imports another application's credentials (notably
  `~/.codex/auth.json`) unless an explicit `--codex-auth` path is given.
- `<install-dir>/.ax/auth.json` is written atomically (temp file + rename).

## Model discovery

Models are discovered dynamically into `<install-dir>/.ax/models/`: a bundled catalog
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
  `<install-dir>/.ax/config.json` (`AxConfig { model: { provider, model, reasoning_effort } }`)
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

## Native WorkBuddy

`ax auth login workbuddy` displays a login URL to click or copy, waits for authorization,
saves access/refresh tokens and expiry in AX's existing `AuthStorage`, and
awaits account model discovery before exiting. The TUI offers WorkBuddy under
`/login` → account sign-in. After login, use `/model` to select a discovered
`workbuddy/<model-id>`; the CLI also accepts `--provider workbuddy --model <id>`.
Regions have separate provider identities, credentials and model caches:

| Region | Provider / credential key | Login command | Model/auth endpoint |
|---|---|---|---|
| International | `workbuddy` | `ax auth login workbuddy --region intl` | `https://www.workbuddy.ai` |
| China | `workbuddy-cn` | `ax auth login workbuddy --region cn` (or `ax auth login workbuddy-cn`) | `https://copilot.tencent.com` |

The original `ax auth login workbuddy` command continues to use International.
`/login` shows **WorkBuddy International** and **WorkBuddy China** separately;
`/model` and persisted model selections use `workbuddy/<id>` or
`workbuddy-cn/<id>`. Both accounts may be signed in concurrently. Signing in
again clears only that region's cache. A China provider never falls back to
International credentials, and known token domain/issuer region mismatches are
rejected before use. `workbuddy-cn --region intl` is rejected as conflicting.
China requests use the China product UA, `www.codebuddy.cn` Origin/Referer,
`copilot.tencent.com` domain and `X-Auth-Refresh-Source: workbuddy`;
International retains `X-Auth-Refresh-Source: plugin`.

Protocol reference: [workbuddy2api-hub](https://github.com/ardeyouxipianyi/workbuddy2api-hub),
commit `6c2a6637f27dcecb6c4956392dfd936fac687306`, verified 2026-09-30.
The current CLI login is a server-issued random state and browser authorization
followed by token polling, **not** an authorization-code localhost redirect.
AX validates the HTTPS authorization origin, login path and exact state
(including rejecting duplicate state parameters), and polls only that attempt.
Neither the reference implementation nor the current CLI login page provides a
localhost callback or PKCE contract. This implementation uses the agreed polling
flow; it does not claim to implement those unsupported features. Temporary
state stays in memory. Login expires after 10 minutes; Ctrl+C cancels CLI login,
Esc cancels a pending TUI login, and leaving the TUI aborts it.

Endpoints:

- `POST /v2/plugin/auth/state?platform=CLI` → `data.state`, `data.authUrl`.
- `GET /v2/plugin/auth/token?state=...`: code `11217` means pending;
  successful `data` carries `accessToken`, `refreshToken`, and expiry (explicit
  epoch, JWT `exp`, or `expiresIn`).
- `POST /v2/plugin/auth/token/refresh`: `X-Refresh-Token` and
  region-specific `X-Auth-Refresh-Source`; accepts both `data` and `data.data` envelopes.
- International `GET /v2/enterprises/personal/models`, China `GET /v3/config`:
  deduplicate `data.agents[].models`
  (object-shaped agents are also accepted), omit the non-chat `lite` channel.
- `POST /v2/chat/completions`: AX messages/tools sent directly, with SSE
  accumulated for non-streaming callers. The shared parser handles split UTF-8,
  LF/CRLF framing, reasoning/tool deltas, repeated `data:` prefixes and `[DONE]`.

Before every model/list request AX reloads its stored credential and refreshes
it when its stored expiry requires renewal. Renewal is serialized across
provider instances and persists rotated tokens before use. HTTP 401 is terminal
and requires renewed authorization rather than replaying the failed request.
403 is terminal. 429 retains Retry-After and enters the runtime retry policy;
neither 403 nor 429 triggers credential renewal. Vendor bodies are sanitized
so token-bearing responses never leak into error output.

WorkBuddy has no invented offline model list. Successful discovery uses the
existing AX model cache and picker; a failed refresh keeps the existing cache
with a warning. A new account login clears the previous WorkBuddy model cache
before discovering its models. Catalog membership is the account's declaration,
not a guarantee against subsequent quota or entitlement changes.

The WorkBuddy product user-agent and product routing headers are required by
the upstream model/catalog protocol. AX sends no machine/session fingerprints,
browser cookies, desktop token imports, proxy rotation, account pooling,
check-ins, rewards, WorkBuddy agent sessions, skills/tools or injected system
prompt. Only AX supplies conversation context and tool schemas.

### Crew account sign-in

ACP catalogs include `auth_kind` (`api_key`, `oauth`, or `ambient`). Crew separates
account sign-in from API keys and environment credentials, and rejects saving
an API key for OAuth providers. `ax auth login openai-codex` also exposes AX's
existing device-code flow to the desktop. Crew runs AX's login command, displays
the URL and device code, and only opens the system browser after a user click.
Closing the authorization dialog cancels the login process. AX remains the
sole owner of token exchange, credential persistence and model discovery.

## Provider retry policy

The runtime classifies typed errors before retrying. HTTP 400/401/403, other
non-429 4xx except 408 and 425, invalid configuration and malformed responses
are terminal. 408 (request timeout), 425 (too early), 429, 5xx, timeout and
connection reset/abort are retryable. No retry occurs after visible text or
thinking has streamed, preventing duplicate output. OAuth renewal based on
stored expiry still happens before sending a request; a failed authenticated
request is not replayed to renew credentials.

`Retry-After` seconds and HTTP-date values take precedence, and every non-2xx
path that can be retried preserves the header — including credential refresh and
catalog requests, not only chat completions. Otherwise AX uses capped
exponential backoff with full jitter. Both attempt and elapsed-time budgets
apply, including a timeout for the remaining retry time. A server wait outside
the budget stops retries instead of shortening its requested wait. Each
successful request resets its retry sequence; model steps/task counts do not
increase for transport retries.

```json
"retry": { "max_attempts":4, "time_budget_ms":30000,
           "base_delay_ms":200, "max_delay_ms":5000 }
```

`max_attempts` includes the initial request; 0/1 disables retries. Missing
configuration maps to these centralized defaults, replacing the old goal-only
retry-once behavior. Ordinary turns and children use the same policy.
