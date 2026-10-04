# ADR 0020: Native model providers and configuration status

Status: Accepted, 2026-10-05

## Context

AX carried pi's provider inventory, but runtime dispatch implemented only Chat
Completions, OpenAI/Codex Responses and WorkBuddy. Saving an Anthropic, Google,
Vertex, Bedrock, Azure, Cloudflare Gateway or Radius key could not create an
adapter. Workers AI already worked with an account id, but missing endpoint
fields were mislabeled as missing support. Catalog and runtime dispatch drifted.

## Decision

- Add native Anthropic Messages, Gemini/Vertex GenerateContent, Bedrock
  ConverseStream and Radius Pi Messages modules behind `ModelProvider`.
  Azure reuses Responses with separate identity, resource routing, deployment
  mapping and `api-key`. Cloudflare Gateway reuses Chat Completions with its
  `/compat` endpoint and `cf-aig-authorization`.
- Runtime, child runtime and discovery share `provider_adapter` construction.
  Existing OpenAI, Codex, DeepSeek and WorkBuddy paths are retained. Workers AI
  inference format remains unchanged.
- Define `supported` by adapter capability. Missing account/resource fields
  have configuration reasons and catalog warnings instead of hiding an adapter.
  Advertise only account OAuth flows AX actually implements.
- Resolve AWS credentials lazily with the official SDK chain and sign with the
  official SigV4 signer. CRC-check event frames with Smithy. Resolve Google ADC
  refresh tokens, service-account JWTs or metadata tokens lazily. No credentials
  are copied from those stores and startup makes no API calls.
- Persist signed assistant blocks in provider/model-scoped optional metadata.
  Replay signatures for the same model and budget their size. Old session JSON
  defaults this field to absent. No extra model calls or transformations.
- Parse native streams as bytes for fragmented UTF-8; reject truncated/error
  streams and preserve status/Retry-After for existing retry policy.
- Radius discovers catalog and inference base through `/v1/config`, not OpenAI.
  Interactive startup without a Radius catalog enters discovery/model selection.

## Consequences and limits

Loopback wire/auth tests prove adapter behavior, not real account access/balance.
Vertex/Bedrock offline catalogs remain fallback/cache. Anthropic/Radius browser
OAuth and Google external-account workload-federation ADC are not implemented.
Vertex covers Google publisher models; Bedrock covers ConverseStream models.
GitHub Copilot remains unsupported. See [providers](../providers.md) for fields.

AWS dependencies are locked for Rust 1.92. SDK credentials/signing coexist with
AX's existing inference HTTP transport and proxy configuration.

Reference: [pi](https://github.com/earendil-works/pi/tree/200387122ca450d6387f033949423114a270b96c/packages/ai/src),
[Cloudflare Unified API](https://developers.cloudflare.com/ai-gateway/usage/chat-completion/),
[Bedrock ConverseStream](https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html),
[Gemini signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures).
