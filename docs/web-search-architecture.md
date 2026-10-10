# Web Search: Two-Tier Native-First Architecture

## Overview

AX provides seamless web search capability through a two-tier automatic routing system:

1. **Native Search (First Priority)**: When the current provider supports built-in web search capabilities, AX uses them directly with no additional configuration required.

2. **Remote Search (Fallback)**: When native search is unavailable, AX automatically routes to configured remote search providers (Brave, Bocha, SearXNG) or falls back to DuckDuckGo.

Users need zero additional configuration for basic search to work. The system automatically detects provider capabilities and routes accordingly.

## Supported Providers with Native Search

The following providers support native web search capabilities:

| Provider | Capability | Notes |
|----------|-----------|-------|
| **OpenAI** | Native | GPT-4, GPT-4o models can access web content through vision/reasoning. Detection is automatic when using these models. |
| **Anthropic** | Native | Claude models may have search capabilities enabled. Routing is automatic based on model version and configuration. |
| **Google Gemini** | Native | Gemini models have built-in search integration. Automatically detected and used when available. |
| **Google Vertex** | Native | Vertex AI Gemini deployments include search. Automatic detection on Google Cloud. |

All other providers use the remote search tier.

## Remote Search Providers

Remote search providers are attempted in this priority order:

### 1. Brave Search (Recommended)
- **URL**: `https://api.search.brave.com/res/v1/web/search`
- **Configuration**: Set `BRAVE_SEARCH_API_KEY` environment variable
- **Characteristics**: Fast, supports 20 results per query, excellent for English-language searches

### 2. Bocha Search (Recommended for Chinese)
- **URL**: `https://api.bocha.cn/v1/web-search` (can be customized)
- **Configuration**: Set `BOCHA_SEARCH_API_KEY` environment variable
- **Characteristics**: Optimized for Chinese language queries, up to 50 results per query

### 3. SearXNG (Self-Hosted)
- **URL**: Custom (set `AX_SEARCH_SEARXNG_URL`)
- **Configuration**: `AX_SEARCH_SEARXNG_URL=https://your-searxng-instance/search`
- **Characteristics**: Privacy-focused, runs locally, no API key needed

### 4. DuckDuckGo (Fallback)
- **URL**: `https://html.duckduckgo.com/html/`
- **Configuration**: None required (always available)
- **Characteristics**: No API key required, HTML parsing-based, slower, limited reliability

## Configuration

### Zero-Configuration Setup (Recommended)

For basic use, no configuration is needed. The system will:
1. Automatically use native search when the current provider supports it
2. Fall back to DuckDuckGo if no other providers are configured

This provides working search for all users out of the box.

### Optional: Add API Keys for Faster Search

To enable faster, more reliable remote search:

```bash
# For Brave Search (recommended)
export BRAVE_SEARCH_API_KEY="your-brave-api-key"

# For Bocha Search (if targeting Chinese queries)
export BOCHA_SEARCH_API_KEY="your-bocha-api-key"

# For self-hosted SearXNG
export AX_SEARCH_SEARXNG_URL="https://your-searxng-instance/search"
```

### Advanced: Custom Timeouts

Each provider's timeout can be individually configured:

```bash
# Connect timeout (default 5s)
export AX_SEARCH_BRAVE_CONNECT_TIMEOUT_MS=10000

# Read timeout (default 10s)
export AX_SEARCH_BRAVE_READ_TIMEOUT_MS=20000

# Total timeout (default 15s)
export AX_SEARCH_BRAVE_TIMEOUT_MS=30000

# Same pattern for BOCHA, SEARXNG, DUCKDUCKGO
```

### Advanced: Circuit Breaker Settings

Control provider failure recovery:

```bash
# Number of consecutive failures before opening circuit (default 3)
AX_SEARCH_CIRCUIT_FAILURE_THRESHOLD=5

# How long to keep circuit open (default 30s)
AX_SEARCH_CIRCUIT_COOLDOWN_SECS=60

# Hedge delay: time to wait before launching second provider (default 500ms)
AX_SEARCH_HEDGE_DELAY_MS=1000
```

## How Search Works

### Single Search Operation

When you ask AX to search for "best Rust web frameworks":

1. **Provider Check**: AX detects your current provider (e.g., `openai`, `anthropic`, `brave`)
2. **Capability Detection**: If the provider supports native search, attempt native search first
3. **Fallback**: If native search unavailable/fails, route to remote providers in priority order
4. **Hedging**: After 500ms with insufficient results, launch a second provider concurrently
5. **Deduplication**: Merge results, remove duplicates by URL
6. **Result**: Return up to the requested limit with provider attribution

### Multi-Query Operation

When searching for 4 queries in one tool call:

```
Queries: ["rust web frameworks", "python async", "golang concurrency", "rust error handling"]
Limit: 20 results per query
```

- All 4 queries run **concurrently** against the same provider
- Results are merged and deduplicated
- As soon as 80 unique URLs are collected (target = 20 × 4), remaining queries are cancelled
- Partial failures are tolerated; if 3/4 queries succeed, the call succeeds

### Resilience & Fallback

The system implements a multi-layer fallback strategy:

```
Current Provider Native Search?
  ↓ (if available)
Try Native Adapter
  ↓ (success or immediate unsupported)
Return Results
  ↓ (if failed or not available)
Remote Provider 1 (Brave/Bocha with circuit breaker)
  ↓ (after hedge_delay or on error)
Remote Provider 2 (SearXNG)
  ↓ (if previous failed)
Remote Provider 3 (DuckDuckGo - always available)
  ↓ (if ALL providers failed)
Return structured error with details for each provider
```

## Error Handling

When search fails:

- **Partial failure** (some queries succeeded): Returns successful results with a note about failed queries
- **Timeout**: Returned as a specific error kind; router moves to next provider
- **API error**: Logged and circuit breaker prevents hammering the same provider
- **All providers down**: Returns `ToolError::Execution` with details about each provider's failure

The model sees all errors and can decide to:
- Refine the query and retry
- Use a different tool (fetch, browser, etc.)
- Report the issue to the user

## Performance Characteristics

### Native Search
- **Latency**: 0.5-2 seconds (depends on provider's implementation)
- **Reliability**: Provider-dependent (usually very high)
- **Cost**: Included with provider subscription (no additional charges)
- **Queries**: Concurrent queries against the same provider
- **Rate limiting**: Provider-dependent

### Remote Search (with Hedging)
- **Brave**: 0.3-1s, highly reliable, ~50 req/s per key
- **Bocha**: 0.5-2s, excellent for Chinese, up to 500 req/day
- **SearXNG**: 1-5s (depends on instance), self-hosted, unlimited
- **DuckDuckGo**: 1-3s, always available, no key required, highest latency

### Hedging
When primary provider is slow:
- After 500ms, a second provider is launched
- Results from both are merged
- Remaining queries of the losing provider are cancelled
- This improves p95 latency significantly

## Architecture

### SearchRouter

The `SearchRouter` in `crates/tool/src/web/router.rs` implements:

- **Provider ordering** based on success rate, latency (EWMA), and fallback status
- **Circuit breaker** pattern: opens after N failures, cooldown prevents hammering
- **Hedging**: concurrent launches with smart cancellation
- **Deduplication**: by URL across all providers
- **Telemetry**: latency, success/failure rates, circuit breaker events

### Telemetry

Search operations emit telemetry:

```
web.search.provider.brave.latency          # Duration
web.search.provider.brave.success          # Count
web.search.provider.brave.failure          # Count
web.search.provider.brave.circuit_open     # When circuit opens
web.search.hedged                          # When hedging triggers
web.search.fallback                        # When fallback is used
```

## Future Enhancements

- **OpenAI native search adapter**: Direct integration with OpenAI's web access
- **Claude native adapter**: Integration with Anthropic's search capabilities
- **Managed search service**: Built-in access to a hosted search backend (like Exa) without requiring user API keys
- **MCP search servers**: Support for Parallel MCP or other MCP-based search implementations
- **Result caching**: Cache recent searches to reduce API calls
- **Search analytics**: Track which searches are most common for insights

## Troubleshooting

### Search is slow
- Check network latency to providers
- If using SearXNG, consider the instance's load
- Enable hedging (default) to improve p95 latency
- Configure timeouts for your network conditions

### "All search providers failed"
- Check your internet connection
- Verify API keys if you've configured them
- DuckDuckGo should always work; if it doesn't, there may be a network issue
- Check if you're behind a firewall/proxy that blocks search providers

### Provider frequently shows as "circuit open"
- Provider may be experiencing issues
- Check provider status page
- Increase `AX_SEARCH_CIRCUIT_COOLDOWN_SECS` if you expect temporary outages

### Native search not being used
- Verify your current provider supports native search (see table above)
- Check that `AX_PROVIDER_ID` is set correctly if using environment-based detection
- Native search may be unavailable on certain model versions

## Related

- See [providers.md](providers.md) for provider authentication setup
- See [tools.md](tools.md) for the `web` tool schema and fetch operations
- See [web-latency.md](web-latency.md) for latency analysis and benchmarks
