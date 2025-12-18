# Token Usage Optimization Research

Date: December 17, 2024

## Overview

This document captures research into Codey's token usage patterns and configuration options for optimization.

## Current Configuration

### Settings Locations

| Setting | Current Value | Location | Configurable |
|---------|---------------|----------|--------------|
| `max_tokens` | 8192 | `config.toml` → `GeneralConfig` | ✅ Yes |
| `thinking_budget` | 16000 | `agent.rs:379` (hardcoded) | ❌ No |
| `compaction_threshold` | 100,000 | `config.toml` → `GeneralConfig` | ✅ Yes |
| `reasoning_effort` | `Budget(16000)` | `agent.rs:411` (hardcoded) | ❌ No |

### Token Flow

1. **Input tokens (context)** - Grows with every message in the conversation
2. **Output tokens** - Capped at `max_tokens` (default: 8192)
3. **Thinking tokens** - Capped at `DEFAULT_THINKING_BUDGET` (16000)
4. **Cache tokens** - Prompt caching is enabled; creation costs tokens, reads save tokens

## ReasoningEffort Enum (genai crate)

The `genai` crate provides a `ReasoningEffort` enum for controlling extended thinking:

```rust
pub enum ReasoningEffort {
    Minimal,      // Disables extended thinking entirely
    Low,          // Light reasoning
    Medium,       // Moderate reasoning
    High,         // Deep reasoning
    Budget(u32),  // Specific token budget
}
```

### Anthropic Provider Mapping

From `lib/genai/src/adapter/adapters/anthropic/adapter_impl.rs`:

```rust
ReasoningEffort::Minimal => None,              // NO thinking - biggest savings!
ReasoningEffort::Low => Some(REASONING_LOW),   // ~2-4k tokens
ReasoningEffort::Medium => Some(REASONING_MEDIUM), // ~8k tokens
ReasoningEffort::High => Some(REASONING_HIGH), // ~16k+ tokens
ReasoningEffort::Budget(budget) => Some(*budget),
```

**Key Finding**: `Minimal` completely disables extended thinking tokens.

## Optimization Recommendations

### 1. Lower `max_tokens` (Easy - config only)

```toml
[general]
max_tokens = 4096  # Half the current value
```

**Trade-off**: Shorter responses, might get cut off on long code generations.

### 2. Make `thinking_budget` / `reasoning_effort` Configurable (Code change required)

Current hardcoded values in `src/llm/agent.rs`:

```rust
// Line 379
const DEFAULT_THINKING_BUDGET: u32 = 16000;

// Line 411
.with_reasoning_effort(ReasoningEffort::Budget(thinking_budget))
```

**Proposed config addition**:

```toml
[general]
# Options: "minimal", "low", "medium", "high", or a number like 8000
reasoning_effort = "medium"
```

**Implementation in `config.rs`**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    // ... existing fields ...
    
    /// Reasoning effort for extended thinking
    /// Options: "minimal", "low", "medium", "high", or a token budget number
    pub reasoning_effort: ReasoningEffortConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReasoningEffortConfig {
    Preset(String),    // "minimal", "low", "medium", "high"
    Budget(u32),       // specific token count
}

impl Default for ReasoningEffortConfig {
    fn default() -> Self {
        Self::Preset("medium".to_string())
    }
}
```

### 3. Lower `compaction_threshold` (Easy - config only)

```toml
[general]
compaction_threshold = 60000  # Compact at 60k instead of 100k
```

**Trade-off**: More frequent compactions, might lose some context detail, but keeps costs lower.

### 4. Use Cheaper Model for Simple Tasks

```toml
[general]
model = "claude-sonnet-4-20250514"  # Instead of opus
```

## Token Savings Estimates

| Change | Estimated Savings | Effort |
|--------|-------------------|--------|
| `reasoning_effort = "minimal"` | Up to 16k tokens/turn | Code change |
| `reasoning_effort = "low"` | ~12k tokens/turn | Code change |
| `reasoning_effort = "medium"` | ~8k tokens/turn | Code change |
| `max_tokens = 4096` | Up to 4k tokens/turn | Config only |
| `compaction_threshold = 60000` | Indirect (earlier reset) | Config only |
| Use Sonnet instead of Opus | ~30-50% cost reduction | Config only |

## Quick Wins (Config Only)

For immediate token reduction without code changes:

```toml
[general]
max_tokens = 4096
compaction_threshold = 60000
model = "claude-sonnet-4-20250514"
```

## Implementation Priority

1. **High Priority**: Make `reasoning_effort` configurable - biggest lever
2. **Medium Priority**: Expose `thinking_budget` as separate config for `Budget(n)` mode
3. **Low Priority**: Add per-request mode switching (simple tasks vs complex tasks)

## Related Code Locations

- `src/config.rs` - Configuration structures
- `src/llm/agent.rs` - Agent and chat options setup
- `config.example.toml` - Example configuration file
- `lib/genai/src/chat/chat_options.rs` - ReasoningEffort enum definition
- `lib/genai/src/adapter/adapters/anthropic/adapter_impl.rs` - Anthropic-specific mapping

## References

- genai crate docs: https://docs.rs/genai/latest/genai/chat/enum.ReasoningEffort.html
- Anthropic extended thinking docs: https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking
