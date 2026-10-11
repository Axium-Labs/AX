# Web Search architecture

AX preserves the unified `web(search)` interface. Kernel tool binding connects
it to the selected concrete model adapter, including registered tools and Mods.
Subagent model overrides rebind native capability; configured clients, remote
sessions and remote health survive child forks.

The production path is:

```
Agent Loop -> permission check -> WebTool::search_many
  -> NativeFirst -> ModelProvider native API/auth/server tools
  -> on unsupported/error/timeout: SearchRouter -> Exa / Parallel MCP
  -> citations/evidence in Tool result -> ordinary Agent model generation
```

Native adapters implement real Responses, Messages, Gemini grounding and
specific documented Chat extensions. Candidates are determined by actual API
endpoint, adapter and credential form, then validated by server execution and
sources. No model-name guessing or universal compatible-API entitlement.
See the complete [provider audit](web-search-audit.md) for the inventory and
protocol evidence, including APIs that do not support their vendor's other
product search features.

Remote defaults are vendor-authorized anonymous Exa and Parallel MCP, initialized
lazily with discovered schemas. No extra search key is required. Optional
Brave/Bocha only enter routing when the user supplies their search key. The
shared scheduler ranks independently by failure history and observed latency,
limits hedging to two requests, honors Retry-After, and recovers after cooldown.
Native runs before Remote, without hedging model-account searches.

Query task budget is 30 seconds, native attempt at most 12 seconds or half the
budget, remote connect/read/total defaults 5/10/15 seconds. Initial native failure
is coalesced across queries. Requests are not replayed; dropped tasks cancel
pending reads/continuations. A remote session 404 invalidates the cached session
for the next task rather than repeating the failed search.

Duplicate queries and canonical URLs merge provenance/citations. Results remain
untrusted external data and cannot grant tool permission. Domain policies fail
closed for hosted search before network access, since visited sites execute on
the remote server. `web(fetch)`, Browser, MCP and Skills retain their separate
execution and permission boundaries.

Service outage produces accurate errors. There is no guaranteed public fallback
and no HTML search-engine parsing. See [quickstart](web-search-quickstart.md),
[implementation](web-search-implementation-guide.md), and [test evidence and
remaining limits](web-search-audit.md#execution-evidence-and-remaining-limits).
