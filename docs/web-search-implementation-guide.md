# Web Search maintenance

The canonical [architecture](web-search-architecture.md) and
[provider audit](web-search-audit.md) replace the former placeholder roadmap.

- `model::ModelProvider` owns `native_search_protocol` and `native_search`.
  Default methods make new/custom adapters Remote-compatible. Add a native
  extension at the adapter boundary, with official API evidence and actual
  source/completion validation; never edit the core router to guess vendor IDs.
- `model::search` owns source/evidence contracts and bounded native transport.
  Native wire adapters reuse existing authentication and streaming parsers.
- `tool::WebTool` owns batching, canonical deduplication and permission handling.
  Its binding/fork hooks preserve selected model and remote state.
- `tool::web::NativeFirst` owns native eligibility/probe cooldown and task budget.
- `tool::web::remote` owns anonymous MCP protocol, negotiated sessions, advertised
  input schemas and actual Exa/Parallel result formats.
- `SearchRouter` owns independent remote health, cooldown, latency, hedging and
  future cancellation. Injected providers/clients remain supported.
- Kernel construction and child model override bind the actual provider; Mods
  must forward binding/fork hooks rather than discard the wrapped tool.

Maintained tests are in `test/web/`. Native TCP fixtures call the production
executor, not a public bypass flag; gateway capability remains false. Registry
classification constructs every adapter. Runtime tests exercise actual kernel
and SubagentManager. Explicit live tests consume existing legitimate credentials
or anonymous public MCP, never baked-in keys. `acp_search.py` validates the real
CLI ACP wire result with DeepSeek. Run live tests explicitly:

```
cargo test -p tool --test web_search live_anonymous_exa_and_parallel -- --ignored --nocapture
cargo test -p model live_openai_native_search -- --ignored --nocapture
python test/web/acp_search.py target/debug/ax.exe
```

The OpenAI test needs a legitimate API key and incurs normal provider usage;
its failure must not be described as a native pass. `AX_HOME` may point to an
existing credential home; live CLI data should use an isolated directory.
Compile, format, Clippy, router/fetch regressions and platform tests are still
required. Record failures and untested account/region environments explicitly
in docs/logs. See the audit for current known Linux harness limitations.
