# Web Search - Quick Start

## Providers with Built-in Search ✓

These providers have native web search and work with AX automatically:

- **OpenAI** (GPT-4, GPT-4o)
- **Anthropic** (Claude models)
- **Google** (Gemini)  
- **Google Vertex** (Gemini on Vertex)

## All Other Providers

Get automatic fallback to remote search (Brave, Bocha, SearXNG, DuckDuckGo).

## Zero Configuration

Just search. It works with any provider, any model.

```bash
ax "search for rust web frameworks"
```

Search is always available.

## Optional: Faster Remote Search

Add an API key for 10x faster, more reliable search:

```bash
# Brave Search (recommended)
export BRAVE_SEARCH_API_KEY="your-key-here"

# Bocha Search (for Chinese queries)
export BOCHA_SEARCH_API_KEY="your-key-here"

# Self-hosted SearXNG
export AX_SEARCH_SEARXNG_URL="https://your-instance/search"
```

Without keys, search still works using DuckDuckGo.

## How It Works

1. **Try native** if your provider supports it
2. **Fall back to remote** if native unavailable or slow
3. **Always fall back to DuckDuckGo** if everything else fails

This happens automatically, you don't need to think about it.

## Common Questions

**Q: Do I need to set up search?**  
A: No. Search works immediately.

**Q: Why is search slow?**  
A: Likely using DuckDuckGo fallback. Add a BRAVE_SEARCH_API_KEY for 10x speedup.

**Q: Which search engine is being used?**  
A: For debugging, check `docs/web-search-architecture.md` - it explains all the backends.

**Q: Can I force a specific search provider?**  
A: Not yet. The system chooses the best available automatically.

## Documentation

- **Full guide**: `docs/web-search-architecture.md`
- **Troubleshooting**: See "Troubleshooting" section in full guide
- **Implementation details**: `docs/web-search-implementation-guide.md`

## That's It

Web search is built-in, automatic, and works out of the box. ✓
