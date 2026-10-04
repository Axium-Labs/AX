# ADR 0019: Search provider routing

Date: 2026-10-04
Status: Accepted

## Context

BuiltinSearch embedded Brave-to-DuckDuckGo fallback, reused fetch's aggressive
2/3/8-second transport limits, and lost history between calls. A single public
HTML engine is unreliable when networking or anti-bot policies change.

## Decision

Retain SearchProvider and add independent Bocha, Brave, SearXNG and DuckDuckGo
adapters. SearchRouter owns configured candidate selection, EWMA latency and
failure history, temporary circuits, fallback and two-in-flight hedging.
DuckDuckGo remains the final keyless fallback. No country detection, model
requests, embeddings, or proactive paid-provider probes are added.

Queries remain concurrent above the router. Provider errors and insufficient
results advance candidates; delayed hedging starts another only when needed.
The common canonical URL merge defines sufficiency before cancellation/limit.
Dropping futures cancels all local work without detached request tasks.

Search creates independent pooled clients with provider-specific timeouts and
no transport retries; fallback owns recovery. Fetch keeps its existing HTTP
semantics, limits, policy checks, error diagnostics and retries. The router is
lazy and shared by calls/clones of one WebTool. Shell search recovery is
explicitly discouraged in tool metadata/errors and the bundled research skill.

## Consequences

Keys/instance URL provide alternatives without changes to the agent loop.
Routing learns only from actual attempts. Untried ties use configuration order;
open circuits can return errors until cooldown. State is per-tool in memory and
is not shared across processes or independent child tools. Injected HTTP clients
keep their own connect/read limits, while the router enforces total deadlines.

Hedging may incur a second provider charge; cancellation cannot undo remote
work. Guidance discourages automatic bypass of search providers, not a global shell
ban. Explicit user requests for alternative search methods or network diagnostics
are allowed, subject to applicable tool permissions.
Paid API reachability and live latency require separately configured credentials;
regression tests use deterministic mocks and loopback HTTP fixtures.
