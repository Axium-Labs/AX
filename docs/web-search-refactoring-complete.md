# Web Search implementation status

The original refactor status overstated empty native adapters as working and
promised a universally available scraped search fallback. Those statements
were incorrect and have been replaced by actual native-first execution and
vendor-authorized hosted MCP search.

See [current architecture](web-search-architecture.md), [complete registry audit,
repairs and execution evidence](web-search-audit.md), and
[maintenance guide](web-search-implementation-guide.md).

Production Remote search is live-verified through DeepSeek CLI, Subagent and ACP,
and on Linux. Native wire adapters are implemented and TCP-tested; native live
credentials are unavailable/invalid in this environment. Full native live
acceptance has **not** been claimed. There are no placeholder success results,
mandatory extra search keys, or guarantees that all networks/services work.
