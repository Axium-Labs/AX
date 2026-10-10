# Web Search Refactoring - Implementation Guide

This document provides guidance for completing the native search adapter implementations and integrating them into AX's web search pipeline.

## Current State (v0.3.10)

✅ **Completed:**
- Two-tier search architecture framework
- Provider capability detection (`SearchCapability` enum)
- Native search adapter trait definition
- Placeholder adapters for OpenAI, Anthropic, Google
- Environment variable support for provider detection (`AX_PROVIDER_ID`)
- Comprehensive documentation and tests
- All existing remote search functionality preserved

⚠️ **To Do:**
- Implement actual OpenAI native search integration
- Implement actual Anthropic native search integration
- Implement actual Google Gemini native search integration
- Integrate native search into the SearchRouter
- Add provider context to WebTool at invocation time
- Add integration tests for fallback scenarios

## Implementation Roadmap

### Phase 1: Router Integration (Next)

**Goal**: Make SearchRouter aware of and use native search capabilities

**File**: `crates/tool/src/web/router.rs`

**Changes Needed**:

1. Add a `native_search` field to SearchRouter:
```rust
pub struct SearchRouter {
    providers: Vec<SearchCandidate>,
    native_search: Option<Box<dyn NativeSearchAdapter>>,
    stats: Arc<Mutex<Vec<ProviderStats>>>,
    // ... existing fields
}
```

2. Update router initialization to create native adapters:
```rust
impl SearchRouter {
    pub async fn new(config: &SearchConfig) -> Self {
        let native = if config.supports_native_search() {
            native::detect_and_create_adapter(config)
        } else {
            None
        };
        
        // ... rest of initialization
    }
}
```

3. Update search method to try native first:
```rust
pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
    // Try native search first
    if let Some(ref adapter) = self.native_search {
        match adapter.search(query, limit).await {
            Ok(Some(results)) => {
                // Record telemetry
                telemetry::record("web.search.native.success", 1.0);
                return Ok(results);
            }
            Ok(None) => {
                // Native search not available, fall through
                telemetry::record("web.search.native.unavailable", 1.0);
            }
            Err(e) => {
                // Native search failed, try remote providers
                telemetry::record("web.search.native.failure", 1.0);
                // Continue to remote search
            }
        }
    }
    
    // Fall through to existing remote search logic
    self.search_remote(query, limit).await
}

async fn search_remote(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, ToolError> {
    // Existing remote search implementation
}
```

### Phase 2: OpenAI Native Search (Quick Win)

**Goal**: Implement OpenAI web search support

**File**: `crates/tool/src/web/native.rs`

**Research Needed**:
- OpenAI's vision-based web retrieval
- Extended thinking web access
- File search / retrieval integration

**Pseudo-code**:
```rust
#[async_trait]
impl NativeSearchAdapter for OpenAiNativeSearch {
    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Option<Vec<SearchResult>>, ToolError> {
        // Option 1: Use vision model with image renders of search results
        // Option 2: Use file_search with web content
        // Option 3: Call OpenAI with a system prompt that enables web browsing
        
        // For now, return None to fall through to remote search
        Ok(None)
    }
}
```

**Status**: Framework ready, implementation deferred pending API clarification

### Phase 3: Anthropic Native Search

**Goal**: Implement Anthropic Claude web search support

**File**: `crates/tool/src/web/native.rs`

**Research Needed**:
- Claude's internal search capabilities
- When are they exposed through the API?
- Does a system prompt enable web access?

**Implementation Strategy**:
- Similar to OpenAI: either via model behavior or explicit API
- Check if we can route search through tool_use instead

**Status**: Placeholder ready

### Phase 4: Google Gemini Native Search

**Goal**: Implement Google Gemini web search support

**File**: `crates/tool/src/web/native.rs`

**Research Needed**:
- Gemini's Grounding API
- Native tools for search
- How to invoke search from the API

**Pseudo-code**:
```rust
#[async_trait]
impl NativeSearchAdapter for GoogleGeminiNativeSearch {
    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Option<Vec<SearchResult>>, ToolError> {
        // Use Gemini's native_tools with search
        // Or use Grounding API for web content
        Ok(None)
    }
}
```

**Status**: Placeholder ready

### Phase 5: Provider Context Integration (Quality)

**Goal**: Make provider information available at tool invocation time

**Motivation**: Currently relying on `AX_PROVIDER_ID` environment variable. This phase would pass provider info directly.

**Files Affected**:
- `crates/core/src/loop_runtime/tool_step.rs`
- `crates/tool/src/web.rs`

**Approach**:
1. Add optional provider context to tool execution environment
2. Update WebTool to accept provider info at execution time
3. Make native search detection more precise

**Status**: Optional enhancement, current environment variable approach works

## Testing Strategy

### Unit Tests (Existing)
- ✅ Native search capability detection
- ✅ Provider enumeration
- ✅ Configuration parsing

### Integration Tests (To Add)
- Test remote search with no native support (fallback path)
- Test native search followed by remote fallback
- Test provider detection from environment
- Test with all major providers (OpenAI, Anthropic, Google, etc.)
- Test timeout and error scenarios

**Location**: `crates/tool/tests/web.rs`

**Example**:
```rust
#[tokio::test]
async fn native_search_falls_back_to_remote_on_failure() {
    // Setup SearchConfig with OpenAI provider
    // Mock native adapter to fail
    // Verify router falls through to Brave search
}

#[tokio::test]
async fn native_search_succeeds_when_available() {
    // Setup SearchConfig with OpenAI provider
    // Mock successful native search
    // Verify results are returned without calling remote providers
}
```

### Manual Testing Checklist
- [ ] Search with OpenAI GPT-4o (should attempt native, fall through)
- [ ] Search with Anthropic Claude (should attempt native, fall through)
- [ ] Search with Google Gemini (should attempt native, fall through)
- [ ] Search with Brave/Bocha API key configured (should use remote)
- [ ] Search with only DuckDuckGo fallback (should work)
- [ ] Search with network failure (should return clear error)
- [ ] Verify circuit breaker behavior

## API Requirements

### For OpenAI Integration
- Need: Web search API endpoint or vision-based retrieval
- Fallback: Use existing remote search + vision for ranking

### For Anthropic Integration  
- Need: Clarification on when Claude models have web access
- Check: Claude system prompt capabilities
- Fallback: Use existing remote search

### For Google Integration
- Need: Grounding API or native_tools documentation
- Check: Vertex AI Gemini search capabilities
- Fallback: Use existing remote search

## Telemetry to Add

```rust
// Native search attempts
telemetry::record("web.search.native.attempted", 1.0);
telemetry::record("web.search.native.success", 1.0);
telemetry::record("web.search.native.failure", 1.0);
telemetry::record("web.search.native.latency", duration_ms);

// Fallback tracking
telemetry::record("web.search.fallback_to_remote", 1.0);
telemetry::record("web.search.tier_used", "native" | "remote");

// Provider-specific
telemetry::record("web.search.native_provider.openai.success", 1.0);
telemetry::record("web.search.native_provider.anthropic.success", 1.0);
telemetry::record("web.search.native_provider.google.success", 1.0);
```

## Error Handling

Native search failures should be:
1. Logged with context (provider, query, error type)
2. Reported to telemetry
3. Handled gracefully with fallback to remote search
4. Never shown to user (transparent tier switch)

```rust
match adapter.search(query, limit).await {
    Ok(Some(results)) => return Ok(results),
    Ok(None) => {
        // Native search not applicable, continue
        telemetry::record("web.search.native.skipped", 1.0);
    }
    Err(e) => {
        // Log and continue to fallback
        telemetry::record("web.search.native.error", 1.0);
        tracing::warn!("Native search failed: {}", e);
    }
}
```

## Configuration for Future

Consider adding these environment variables for debugging:

```bash
# Force use of specific tier
AX_SEARCH_TIER=native|remote|auto  # default: auto

# Disable native search (force remote)
AX_SEARCH_DISABLE_NATIVE=1

# Native search timeout
AX_SEARCH_NATIVE_TIMEOUT_MS=5000   # default: 3000

# Debug logging
AX_SEARCH_DEBUG=1
```

## Documentation to Update

As implementations are completed:

1. [web-search-architecture.md](../docs/web-search-architecture.md) - Implementation status
2. [tools.md](../docs/tools.md) - Native search availability per provider
3. [providers.md](../docs/providers.md) - Update with native search status
4. Release notes - Document native search when available
5. [architecture.md](../docs/architecture.md) - Add native search as a subsystem

## Notes on Each Provider

### OpenAI
- GPT-4 and later support web browsing via vision
- Extended thinking models may have web access
- Might need to use file_search + web content retrieval
- Test with: `gpt-4o`, `gpt-4-turbo`

### Anthropic  
- Claude models don't have explicit web search API
- May have internal search capability
- Check latest Claude model capabilities
- Test with: `claude-opus-4`, `claude-sonnet`

### Google
- Gemini has Grounding API for web search
- Native tools may include search
- Vertex AI integration available
- Test with: `gemini-2.0-pro`, `gemini-1.5-pro`

## Success Criteria

- [ ] Two-tier search works end-to-end
- [ ] Native search is transparent (user doesn't care which tier is used)
- [ ] Fallback is automatic and reliable
- [ ] All existing tests still pass
- [ ] No breaking changes to web tool API
- [ ] Documentation is complete
- [ ] Telemetry shows tier usage

## Questions & Future Considerations

1. Should native search tier be configurable? (e.g., force remote for consistency)
2. Should we cache native search results differently than remote?
3. Should native search have different rate limiting?
4. How to handle mixed results from multiple tiers?
5. Should we expose which tier was used to the model/user?

See [web-search-architecture.md](../docs/web-search-architecture.md) for more context.
