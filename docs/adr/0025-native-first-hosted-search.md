# 0025: Native-first hosted search

Status: Accepted, 2026-10-11. Supersedes the backend selection in ADR 0019.

## Decision

Keep `web(search)` and the existing Agent Loop/remote scheduler. Bind the actual
model adapter into tools at kernel construction and child model replacement.
The adapter owns native API/authentication and source validation; compatible
interfaces do not imply OpenAI search support. Native responses become tool
evidence for the ordinary subsequent Agent generation.

Fallback uses anonymous, vendor-authorized Exa/Parallel MCP with real schema
discovery, bounded requests and shared independent health. Explicit user-key
Brave/Bocha remain optional. Remove HTML engine scraping and SearXNG. Native
search is sequential before Remote; model hedging never duplicates it.

## Consequences

All runnable models share Remote access without a separate search key. Native
support remains account/model/endpoint-specific and must be validated by actual
server execution. Search has cancellation/deadline limits and preserves citation
provenance. Domain-constrained server search fails closed. Anonymous service
availability and semantic prompt-injection resistance are not guaranteed.

See [audit](../web-search-audit.md) and [architecture](../web-search-architecture.md).

Amendment, 2026-10-29: anonymous Keenable MCP (`https://api.keenable.ai/mcp`)
is a third vendor-authorized Remote. It is a router fallback candidate, so it
is tried only when Exa and Parallel fail, return too few results or are cooling
down; lower observed latency never ranks it ahead of them. Requests carry the
documented `X-Keenable-Title: AX` client label and no key. Endpoint `None`
disables it like the other remotes.
