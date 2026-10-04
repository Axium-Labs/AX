# 2026-10-04 — Search provider routing

Task: Remove DuckDuckGo single-provider dependence and tolerate unreliable local networking without changing known-URL fetch.

Changed:
- crates/tool/src/web/providers.rs: retained SearchProvider contract; independent Bocha, Brave, SearXNG and DuckDuckGo adapters normalize title/url/snippet/source. Bocha read-only POST has no summary/model call; SearXNG requests JSON. Existing DOM/redirect/ad filtering regression moved to test/web.
- crates/tool/src/web/router.rs: configured candidate list; recent consecutive failures then measured EWMA latency order, DuckDuckGo last; bounded two-in-flight delayed hedging; immediate fallback on transport/status/challenge/API errors or short results; shared canonical URL merge and final limit. Sufficient results and caller cancellation drop remaining futures.
- Router stats/telemetry record successes, failures, latency, cancellation and circuit opens. Default three consecutive failures open a 30-second circuit; cooldown allows retry and successful retry clears failures. State/client pools persist across calls and WebTool clones. Configuration/client replacement resets router state.
- network.rs: independent search clients, default connect/read/total 5/10/15 seconds, per-provider AX_SEARCH_<PROVIDER>_*TIMEOUT_MS env overrides. Search does not retry transport errors; router handles recovery. Fetch client, GET/content/redirect/permission/size semantics and retries unchanged.
- web.rs/lib.rs: lazy reusable router integration and exports; preserved queries concurrency above router and custom provider/client injection. Injected client connect/read settings stay intact, with independent router total deadlines.
- Tool description, all-failed error and skills/web-research/SKILL.md discourage automatic shell/Python urllib search recovery and retain fetch for known URLs. This is model-facing guidance, not a global shell permission restriction.
- test/web/search_router.rs: 18 regression tests registered in crates/tool/Cargo.toml. Existing preference fixture requests its one sufficient result explicitly.
- docs/tools.md, architecture.md and ADR 0019 describe behavior, config and limitations; ADR index updated. Website checked: no engine-specific claims affected. AXCrew and website code unchanged.

Why:
- Provider protocol parsing must not own fallback; locally observed reachability/latency should determine routing. Avoid default fan-out to all paid providers and preserve the existing fetch contract.

Validation:
- cargo test --workspace: PASS, 565 passed / 0 failed / 3 ignored (included initial 16 router regressions).
- After adding configured HTTP timeout/caller cancellation tests and source lint cleanup: cargo test -p tool PASS, 104 passed / 0 failed, including 18 router regressions and 27 existing web tests.
- cargo clippy --workspace --all-targets: PASS; no new router/provider/test warnings; existing workspace warnings remain.
- cargo fmt --all --check and git diff --check: PASS.
- Local fixtures verify API methods/auth/query/body, real DNS/connection errors, HTTP 429/5xx without retry, malformed/challenge pages, timeout isolation, circuit skip/retry/success recovery, shared state across clones, EWMA ordering, no unnecessary paid hedge, cancellation and canonical deduplication/limit.

Limitations / Next:
- Routing history is in memory per WebTool, not persisted across processes or independently constructed child tools. No proactive probes; cold ties use Bocha/Brave/SearXNG configuration order.
- SearXNG must enable JSON. Short/empty successful responses are valid after exhausting candidates; cancellation cannot undo provider charges or work already received.
- No live paid-provider calls, public-network latency benchmark, commit, release or installation performed.
- No required implementation work remains.
