# Web scheduling latency benchmark

Measured on 2026-09-30 in the local Windows development environment.

```powershell
cargo run -p tool --example web_latency
cargo run -p tool --example web_latency -- --live
```

The reproducible benchmark compares the previous concurrent `join_all`
wait-for-all strategy with the new completion-driven scheduler. Both paths
have the same three queries/URLs, shared client and target of one useful
result. One request takes 20 ms and two stragglers take 300 ms. Fetch uses a
loopback HTTP server; search uses an asynchronous provider fixture. There is
one warmup round and 30 recorded rounds. P50/P95 use nearest-rank percentiles.
The development profile and Windows timer granularity are included in the
measured values. [Raw benchmark output](web-latency.json) is saved alongside
this report.

| Operation | Before P50 | Before P95 | After P50 | After P95 |
|---|---:|---:|---:|---:|
| Search | 309.15 ms | 311.70 ms | 31.08 ms | 32.18 ms |
| Fetch | 309.97 ms | 312.85 ms | 31.21 ms | 31.86 ms |

This is approximately a 90% reduction for this sufficient-result workload.
It measures scheduling and cancellation under controlled stragglers, not a
promise about public search-engine latency. Fetching every requested page
still waits for every page; use `target_pages` when only a subset is needed.
Connection pooling is independently tested with two successive built-in
search requests reusing one Keep-Alive connection. DNS cache tests verify
coalescing and reuse, and retry tests verify 401/403/404 are attempted once and
429/5xx at most twice.

The opt-in live smoke test explicitly uses no Brave key and retrieved three
DuckDuckGo HTML results for a public Rust query, with no destination-body
fetches. Its observed total was 1758 ms and response-header arrival was
1751 ms. These are single-run observations, separate from benchmark percentiles.

`/status` exposes measured DNS, connector, response-header/TTFB and total
durations. The connector measurement includes DNS/proxy/TLS as applicable;
response-header arrival is the available reqwest approximation of TTFB.
Phases overlap and are not additive. Cache hits and pooled connections may
avoid DNS or connection creation entirely; no timings are invented for them.
