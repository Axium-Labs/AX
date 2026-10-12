# Web Search production audit

Audited 2026-10-10–11 against the current registry, model catalog/discovery,
credential resolution, adapter factory, kernel, child composition, CLI and ACP.
This report distinguishes protocol candidates from proven account/model access.

## Production path

CLI/TUI/ACP composition creates the selected `ModelProvider` and `WebTool`.
`AgentKernel::new`/`register_tool` bind the adapter through `Tool::bind_model`;
Mods forward that hook. The ordinary Agent Loop performs permission checks,
then `WebTool::search_many` runs `NativeFirst` for each unique query. Native
search uses the adapter's actual protocol and authentication. A successful
server search supplies sources and retained platform evidence to the ordinary
tool result; the same Agent Loop submits it to the selected model for its final
answer. Research completions are separate from the parent conversation, so
server tools are never mistaken for AX client function calls.

If native search is unsupported, rejected, malformed, source-less or times out,
`NativeFirst` calls the shared `SearchRouter`, which runs Exa/Parallel MCP.
The existing latency ranking, bounded hedging, circuits and cancellation remain.
Subagent forks preserve configured clients, routing state and search sessions.
A child model override rebinds native capability to that child's model; remote
health remains shared. Model hedging never launches duplicate native searches.
AXCrew's AX integration consumes this same ACP runtime. No Crew source change
was necessary; the real ACP wire path was tested, not the desktop UI.

## Capability rules

`SearchCapability::ProtocolCandidate` is a registry hint, not proof of support.
Routing reads `ModelProvider::native_search_protocol`, not registry IDs or model
names. It checks HTTPS host, API path, adapter and credential form. Compatible
gateways have no default OpenAI search entitlement. Actual server responses must
contain usable HTTP(S) sources and completion/execution evidence. The first
account/model probe is serialized; failure cools down subsequent probes. Model
replacement resets native state. Unsupported 400/404/422 disables further
attempts for the binding. 401/403/429 and repeated failures enter cooldown;
Retry-After is preserved. Remote search still requires the selected model's own
inference credentials to be usable; it cannot repair expired model login.

Catalog-discovered model IDs use the same adapter and server validation; no
static model-family allowlist is required. A custom/dynamically added adapter
inherits Remote by default, or implements both native trait methods for its
own documented protocol. The current CLI has no custom-provider registration
feature; this audit does not reintroduce the feature removed on 2026-10-09.

## Complete registry classification

The following inventory is exhaustive for this checkout. “Candidate” always
means conditional on the actual endpoint, model, region and account entitlement.
All runnable entries have Remote fallback; Copilot is catalog-only and cannot
run an AX Agent in this checkout.

Registry entries: **43**, including one non-runnable catalog entry.

| Provider ID | Current AX API | Auth | Native / fallback basis |
|---|---|---|---|
| `workbuddy-cn` | Managed | ExternalOAuth | Remote: separate China OAuth/catalog transport, no verified server-search contract. |
| `workbuddy` | Managed | ExternalOAuth | Remote: International managed OAuth chat transport has no documented server-search extension. |
| `amazon-bedrock` | Bedrock | Ambient | Remote: current ConverseStream has no search contract; Mantle Responses is a different API. |
| `ant-ling` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `anthropic` | Anthropic | ApiKey | Candidate: official Messages/key/server web_search; custom gateway uses Remote. |
| `azure-openai-responses` | OpenAiResponses | ApiKey | Candidate: official Azure Responses deployment/api-key; entitlement and region checked by request. |
| `baseten` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `cerebras` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `cloudflare-ai-gateway` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `cloudflare-workers-ai` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `deepseek` | OpenAiCompatible | ApiKey | Remote: actual AX Chat Completions supports client functions; live tested. |
| `fireworks` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `github-copilot` | Managed | ExternalOAuth | Not runnable: registry entry has no AX runtime adapter; cannot execute an Agent. |
| `google` | Google | ApiKey | Candidate: official Gemini API/key/google_search; custom gateway uses Remote. |
| `google-vertex` | Google | Ambient | Candidate: Vertex publishers/google endpoint; existing ADC or express key and region. |
| `groq` | OpenAiCompatible | ApiKey | Candidate: official global Chat browser_search; require executed tool/source receipt. |
| `huggingface` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `kimi-coding` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `meta` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `minimax` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `minimax-cn` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `mistral` | OpenAiCompatible | ApiKey | Remote: search requires Conversations/Agents rather than current Chat Completions. |
| `moonshotai` | OpenAiCompatible | ApiKey | Candidate: official Moonshot builtin $web_search with echo continuation. |
| `moonshotai-cn` | OpenAiCompatible | ApiKey | Candidate: independent official China endpoint/key; same builtin protocol. |
| `nvidia` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `openai` | OpenAiResponses | ApiKey | Candidate: official Responses; completed web_search_call and citations required. |
| `openai-codex` | OpenAiResponses | CodexOAuth | Candidate: actual ChatGPT Codex Responses OAuth; not OpenAI API-key access. |
| `opencode` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `opencode-go` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `openrouter` | OpenAiCompatible | ApiKey | Candidate: official API plugin engine=native only; no paid remote engine fallback. |
| `qwen-token-plan` | OpenAiCompatible | ApiKey | Remote: Token Plan Chat/Harness contract is distinct from general Model Studio enable_search/source API. |
| `qwen-token-plan-cn` | OpenAiCompatible | ApiKey | Remote: Token Plan Chat/Harness contract is distinct from general Model Studio enable_search/source API. |
| `qwen-token-plan-individual` | OpenAiCompatible | ApiKey | Remote: Token Plan Chat/Harness contract is distinct from general Model Studio enable_search/source API. |
| `radius` | PiMessages | ApiKey | Remote: current Pi Messages adapter has no verified server-search contract. |
| `together` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `vercel-ai-gateway` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `xai` | OpenAiCompatible | ApiKey | Remote: current Chat Completions; old Live Search retired, Responses is a different protocol. |
| `xiaomi` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `xiaomi-token-plan-ams` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `xiaomi-token-plan-cn` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `xiaomi-token-plan-sgp` | OpenAiCompatible | ApiKey | Remote: current Chat Completions adapter has no verified server-search extension; never infer from model name. |
| `zai` | OpenAiCompatible | ApiKey | Remote by default: coding endpoint. Ordinary official /api/paas/v4 override is a conditional native candidate. |
| `zai-coding-cn` | OpenAiCompatible | ApiKey | Remote by default: China coding endpoint; ordinary BigModel API is distinct. |

## Official protocol evidence

- [OpenAI Responses](https://developers.openai.com/api/docs/guides/tools-web-search):
  `web_search` plus completed search receipt and URL annotations/action sources.
  Codex OAuth uses its existing ChatGPT Responses transport and preview tool;
  a ChatGPT OAuth token is not treated as a normal OpenAI API key.
- [Anthropic Messages](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool):
  `web_search_20250305`, server result blocks, citations, encrypted content and
  usage. Paused/incomplete responses fail over, not an unbounded continuation.
- [Gemini grounding](https://ai.google.dev/gemini-api/docs/google-search):
  `google_search`, grounding chunks, supports and search entry point. Vertex
  reuses its existing API-key/ADC refresh, quota project and regional endpoint.
- [Azure](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/web-search):
  actual Azure Responses deployment and `api-key`; regional/admin rejection
  falls back. A matching model name does not grant deployment support.
- [Moonshot](https://platform.kimi.ai/docs/guide/use-web-search): builtin
  `$web_search`, unchanged assistant reasoning/tool arguments and one echo
  continuation. Markdown references are labeled as builtin-answer references;
  AX does not invent OpenAI annotations for this protocol.
- [OpenRouter](https://openrouter.ai/docs/guides/features/plugins/web-search):
  explicitly `engine: native`, retained URL annotations. AX never selects auto,
  Exa, Parallel or another credit-billed OpenRouter search engine.
- [Groq](https://console.groq.com/docs/tool-use/built-in-tools/browser-search):
  required `browser_search`, executed tool/browser/search results and citations.
  Unsupported models and regional/sovereign endpoints use Remote. Removed
  Compound models are not selected as replacements.
- [Z.AI](https://docs.z.ai/guides/tools/web-search): ordinary `/api/paas/v4`
  Chat search and `web_search` sources. The AX default `/api/coding/paas/v4`
  does not inherit this capability or silently switch to the paid general API.
- [Model Studio](https://docs.modelstudio.console.alibabacloud.com/en/model-studio/web-search):
  general API `enable_search` with actual returned source metadata. AX's
  [Token Plan Harness](https://www.alibabacloud.com/help/en/model-studio/token-plan-harness-tool)
  is a separate automatic-tool contract; it is not falsely classified from the
  general API documentation. Current Token Plan Chat endpoints use Remote.
- [Mistral](https://docs.mistral.ai/studio/agents/agent-tools/websearch) requires
  Conversations/Agents, not AX's Chat Completions adapter.
  [Bedrock](https://docs.aws.amazon.com/bedrock/latest/userguide/web-search.html)
  search uses Mantle Responses, not the current ConverseStream adapter.
  [DeepSeek Chat](https://api-docs.deepseek.com/api/create-chat-completion/)
  supports client function calls, so AX uses Remote.

Native model-account searches can incur that provider's inference/search usage.
This is disclosed in tool guidance and quickstart. There are no bundled private
keys. Optional Brave/Bocha require explicitly configured user search keys.

## Remote service authorization

[Exa MCP](https://exa.ai/mcp) documents keyless client use at
`https://mcp.exa.ai/mcp`.
[Parallel's announcement](https://parallel.ai/blog/free-web-search-mcp) and
[MCP documentation](https://docs.parallel.ai/integrations/mcp/search-mcp)
authorize anonymous clients at `https://search.parallel.ai/mcp`.
AX sends no API key, account cookies, paid SKU or spoofed model identity.
It initializes MCP, negotiates a supported version, discovers the current tool
schema and calls the advertised search tool. Exa's current required `objective`
is discovered rather than assumed from an old example. Sessions and Parallel
conversation IDs are reused. Expired sessions are reinitialized on the next
task; the failed search is not replayed.

These are third-party services with network/anonymous rate limits and no AX
availability guarantee. Complete outage produces an accurate tool error.
An explicit structured empty results array is a valid empty search, not fake
success with invented sources. Real source relevance can vary, particularly
between backends; users should prefer authoritative results and fetch sources
when needed.

## Repairs and safety

Removed all DuckDuckGo page scraping, DOM/redirect extraction, SearXNG adapters,
their configuration/timeouts and obsolete tests; removed direct DOM dependencies
used only for search. HTML-to-Markdown dependencies remain for `web(fetch)`.
Removed current-provider/environment guessing and empty native adapter returns.
Replaced incorrect “complete/always reliable” documentation with actual behavior.

Native has a 12-second maximum attempt; total query task budget defaults to
30 seconds and always reserves time for Remote. Remote attempts default to
5/10/15-second connect/read/total limits. HTTP 429 honors Retry-After; failures
and latency are independent per remote backend. OpenAI/Azure API research
requests cap output at 2048 tokens and built-in tool calls at one; these controls
are deliberately omitted for the distinct Codex subscription transport. Other
native adapters cap research output at 2048 tokens and Moonshot allows only one
continuation. Full answers and original platform evidence are retained; per-source excerpts
are bounded rather than repeating the full answer for every URL. At most two Remote requests
are in flight per query. Dropping a task/hedged request cancels local futures,
reads and pending continuation work. No detached searches or transport replays
are launched. Servers may already have received work when cancellation occurs.

TLS/proxy behavior uses reqwest's normal environment/OS configuration. Search
rejects redirects and sanitizes HTTP errors. MCP/JSON responses and accumulated
native events are bounded to 2 MB; the shared native SSE transport additionally
caps an individual pending frame at 8 MiB. Duplicate
queries and canonical URLs are deduplicated; citation/evidence provenance from
every completed duplicate is merged. Output is marked `untrusted_external_data`;
model guidance rejects retrieved instructions. This helps preserve the existing
permissions, not a claim of perfect semantic prompt-injection prevention.
Domain-restricted search fails closed before any provider call, since a hosted
server's visited sites cannot be enforced locally. Known-URL fetch keeps its
existing separate redirect/domain permission checks.

## Execution evidence and remaining limits

New maintained tests live in `test/web/`: TCP/SSE native protocols, real MCP wire
handshake and tool calls, entire registry classification, native→remote errors,
401/403/429/503, cooldown/recovery, expired session without replay, proxy routing,
query deduplication, cancellation and deadlines; actual AgentKernel/forked child
and SubagentManager model override. Existing router and fetch regressions remain.
Windows full-workspace serial tests pass (689 passed, 6 ignored); formatting,
workspace compilation and all-target Clippy pass (one existing CLI warning).
Linux/WSL full-workspace serial tests also pass (686 passed, 6 ignored, plus the
real CLI Strict-sandbox target). Workspace compilation and all-target Clippy
pass on both platforms; Linux retains two existing desktop warnings in addition
to the CLI warning. All 62 search-specific tests pass on both platforms. See `logs/2026-10-11-web-search-audit.md` for
commands, intermediate concurrent-edit failures and final outcomes.

Real authorized runs succeeded: anonymous Exa and Parallel on Windows and Linux
(WSL Ubuntu); Windows DeepSeek CLI Agent Loop, real isolated Subagent and ACP;
Linux DeepSeek CLI and isolated Subagent. Parent/child session tool records were inspected to confirm
actual `web` calls and backend sources, beyond final model text.

No native service has a successful live credential check in this environment:
the available OpenAI key returns 401; existing Codex OAuth refresh returns 401;
no authorized Anthropic/Gemini/Vertex/Azure/Moonshot/OpenRouter/Groq/general
Z.AI/Model Studio credentials were found. TCP protocol fixtures do not establish
live model entitlement. These paths are implemented but their live acceptance
remains unverified. DeepSeek validates Remote, not Native.

Linux validation infrastructure was repaired rather than suppressing failures:
the obsolete private-module `linux_child_sandbox` target was replaced by
`test/web/linux_runtime.rs`, which launches the real AX CLI with production
composition, a TCP model fixture, Strict sandbox and hosted MCP fixtures. It
checks dirty Git inheritance/commit, denied host credentials and private child
SQLite access, actual Subagent search, retained JSONL citations and terminal
workspace cleanup. Lifecycle unit fixtures explicitly initialize their existing
Off policy once; they no longer try to execute a libtest program as a broker.
The backup rollback test removes its deliberately created directory obstruction
before checking database/session rollback, respecting Linux JSONL recovery.
Original failures remain recorded in logs as intermediate audit evidence.

Native
Linux hosts beyond WSL, Windows proxy environment switching, authenticated/SOCKS
proxies, domestic/international reachability permutations, paid Brave/Bocha and
the actual AXCrew desktop UI were not live-tested. Loopback explicit HTTP proxy replacement, DNS/TLS failures,
timeouts, permissions and cancellation are tested on both OS builds. Proxy
environment changes require rebuilding clients/restarting AX; they are not
hot-reloaded into existing model and Remote connections.

Ordinary users need no extra **search** key for the authorized anonymous Remote
services, and this is proven through production execution. This does not imply
free model inference, universal network availability or full native-provider
live acceptance. The complete requested live-validation matrix is not yet met.
