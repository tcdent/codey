# Fast Mode Implementation Guide

Working reference for implementing fast mode in codey. For reverse engineering
details and algorithm derivations, see `fast-mode.md`.

## Architecture

```
src/llm/
├── provider/
│   ├── mod.rs          — RequestCustomizer trait
│   ├── anthropic.rs    — AnthropicKey, AnthropicOAuth, AnthropicFastMode + hash fns
│   └── openrouter.rs   — OpenRouter
├── agent.rs            — provider-agnostic agent loop
├── client.rs           — genai Client builder
└── mod.rs              — module registration
```

The `RequestCustomizer` trait lets each provider customize the request:
- `label()` → provider name for logging
- `apply_chat_options(options, thinking_budget)` → headers, extra_body, body_serializer, reasoning
- `apply_chat_request(request)` → system prompt injection, message modification (default: no-op)

## What's Implemented

### Headers ✅
`AnthropicFastMode::apply_chat_options` sets all required headers:
- `authorization: Bearer {token}`
- `anthropic-beta: claude-code-20250219,oauth-2025-04-20,context-management-2025-06-27,prompt-caching-scope-2026-01-05,effort-2025-11-24,adaptive-thinking-2026-01-28,research-preview-2026-02-01`
- `anthropic-version: 2023-06-01`
- `anthropic-dangerous-direct-browser-access: true`
- `user-agent: claude-cli/2.1.37 (external, cli)`
- `x-app: cli`

### Reasoning ✅
`ReasoningEffort::Adaptive` → genai emits `{"thinking": {"type": "adaptive"}}`

### Cooldown ✅
`cooldown_on_rate_limit()` returns true. Agent enters 20-minute cooldown on 429/529,
falls back to `AnthropicOAuth` for subsequent requests.

### Hash Functions ✅
Pure functions in `provider/anthropic.rs`, all tested:
- `compute_cch(body, seed)` — xxHash64 & 0xFFFFF → 5-char hex
- `compute_cc_version_suffix(first_user_msg, salt, version)` — sha256 → 3-char hex
- `build_billing_header(first_user_msg, salt, version)` → full header string with `cch=00000`
- `apply_cch(body, seed)` → replaces placeholder with computed hash
- `build_user_id(user_id, account_uuid, session_id)` → metadata string

## What's Missing

### 1. Config Fields

`AgentRuntimeConfig` needs sensitive fast mode parameters:

```rust
pub struct AgentRuntimeConfig {
    // ... existing fields ...
    pub cch_seed: Option<u64>,
    pub cc_version_salt: Option<String>,
}
```

Config file (`~/.config/codey/config.toml`):
```toml
[agents.foreground]
fast_mode = true
cch_seed = 0x6e52736ac806831e
cc_version_salt = "59cf53e54c78"
```

These MUST NOT be hardcoded in source. They're extracted from the Claude Code
binary and should only exist in the user's local config.

### 2. Metadata Construction (user_id)

The `metadata.user_id` field in the request body identifies the billing account.

**Format:** `user_{user_id}_account_{account_uuid}_session_{session_id}`

| Component | Source | Persistence |
|-----------|--------|-------------|
| `user_id` | Random 32-byte hex string | Generated once, persisted in config/auth |
| `account_uuid` | From OAuth profile response | Stored after OAuth flow |
| `session_id` | UUID v4 | Generated per codey session (in-memory) |

**Where this data lives today:**
- `user_id` → Doesn't exist yet. Needs to be generated once and persisted.
  Could go in `~/.config/codey/auth.json` alongside OAuth creds, or in a
  separate identity file.
- `account_uuid` → Not currently captured. The OAuth profile endpoint
  (`GET /api/oauth/profile`) returns this. We'd need to fetch and store it
  during the OAuth flow or on first use.
- `session_id` → Generate at `Agent::new()` time, store as field. `uuid::Uuid::new_v4()`.

**Implementation plan:**
- Add `account_uuid: Option<String>` to `OAuthCredentials` (stored in auth.json)
- Add `user_id: String` to a persistent identity store (generate once with `hex::encode(rand::random::<[u8; 32]>())`)
- Add `session_id: String` as a field on `Agent` (fresh UUID v4 per session)
- `AnthropicFastMode` struct gets these fields, calls `build_user_id()` in `apply_chat_options`
- Passed as `metadata: {"user_id": "..."}` in `extra_body`

### 3. System Prompt Structure

The `system` field in the request body is an array of text blocks. The billing
header must be the **first block**, with no `cache_control`. The golden capture
(mitm_raw_061) shows this exact structure:

```json
"system": [
  {
    "type": "text",
    "text": "x-anthropic-billing-header: cc_version=2.1.37.fbe; cc_entrypoint=cli; cch=a112b;"
  },
  {
    "type": "text",
    "text": "You are Claude Code, Anthropic's official CLI for Claude."
  },
  {
    "type": "text",
    "text": "\nYou are an interactive agent that helps users with...",
    "cache_control": {"type": "ephemeral"}
  }
]
```

**Key details:**
- Block [0]: billing header — **no `cache_control`**
- Block [1]: identity line — **no `cache_control`**
- Block [2]: main system prompt — **has `cache_control: ephemeral`** (last block only)
- The `cch=00000` placeholder in the billing header is replaced by `CchSerializer`
  after the full payload is serialized

**Implementation:** The `CchSerializer` body serializer handles billing header
injection. It receives the finalized JSON payload from genai, inserts the billing
header as `system[0]`, then serializes and applies the cch hash. This bypasses
genai's system prompt construction entirely for the billing header block.

genai's "You are Claude Code" prepend has been **removed** from `adapter_impl.rs`.
Codey controls its own system prompt content via the system message in `messages[0]`.

### 4. Extra Body Params

**Both `AnthropicOAuth` and `AnthropicFastMode`** include via `with_extra_body()`:
```json
{
  "metadata": {
    "user_id": "user_{hash}_account_{uuid}_session_{uuid}"
  }
}
```

**`AnthropicFastMode` only** adds:
```json
{
  "research_preview_2026_02": "active",
  "context_management": {
    "edits": [{"type": "clear_thinking_20251015", "keep": "all"}]
  }
}
```

**Normal OAuth (072 golden) does NOT include** `context_management` or
`research_preview_2026_02`. It does include `output_config` and `temperature`
which fast mode omits.

### 5. CchSerializer (Body Serializer)

A struct implementing genai's `BodySerializer` trait that handles two jobs:
1. **Billing header injection** — inserts the billing header as `system[0]`
2. **cch hash computation** — replaces the `cch=00000` placeholder with the real hash

```rust
struct CchSerializer {
    seed: u64,
    billing_header: Option<String>,
}

impl BodySerializer for CchSerializer {
    fn serialize(&self, payload: &Value) -> Result<String, String> {
        let mut payload = payload.clone();
        // 1. Insert billing header as system[0] (no cache_control)
        if let Some(ref header) = self.billing_header {
            let block = json!({"type": "text", "text": header});
            // Handle system as array or string
            match payload.get_mut("system") { ... }
        }
        // 2. Serialize and replace cch placeholder
        let body = serde_json::to_string(&payload)?;
        Ok(apply_cch(&body, self.seed))
    }
}
```

**Data flow:**
1. genai builds the full JSON `Value` payload (model, messages, system, tools, extra_body)
2. Instead of `reqwest.json(&payload)`, calls `serializer.serialize(&payload)`
3. `CchSerializer` inserts billing header at `system[0]`, serializes, replaces `cch=00000`
4. genai sends the raw string body with `content-type: application/json`

The billing header is built in `apply_chat_request` and passed to the serializer.
This approach bypasses genai's system prompt construction for the billing block,
ensuring it's always first regardless of how genai orders other system content.

### 6. genai Changes ✅

The "You are Claude Code" system prompt prepend has been removed from
`adapter_impl.rs`. Codey controls its own system prompt entirely via
`ChatMessage::system()` in the messages array. The `CchSerializer` handles
billing header injection at the payload level.

## max_tokens Override

**Status: needs research.** Golden capture shows `max_tokens: 32000`. Unknown
whether this is:
- Required by adaptive thinking (server rejects lower values?)
- Just the Claude Code default for Opus 4.6
- Part of the fast mode gate

Current codey default is 8192 via config. If it turns out to be required,
add `max_tokens` override to `apply_chat_options` or bump the config default.

## Stainless Headers

**Status: unknown if required.** Golden capture includes:
```
X-Stainless-Lang: js
X-Stainless-Package-Version: 0.70.0
X-Stainless-OS: MacOS
X-Stainless-Arch: arm64
X-Stainless-Runtime: node
X-Stainless-Runtime-Version: v24.3.0
X-Stainless-Timeout: 600
```

These are added by the Anthropic JS SDK (`@anthropic-ai/sdk`). They're
telemetry/debugging headers. Unlikely to be part of the server-side gate
since the cch hash is the verified access control. Deprioritized.

## Implementation Order

1. **Config fields** — Add `cch_seed`, `cc_version_salt` to config parser
2. **Metadata user_id** — Generate/persist identity, add session_id to Agent
3. **System prompt** — Move "Claude Code" prepend from genai to codey providers,
   add billing header injection for fast mode
4. **Extra body params** — Wire up `with_extra_body()` in `AnthropicFastMode`
5. **CchSerializer** — Create and wire up body serializer
6. **genai cleanup** — Remove OAuth system prepend from genai adapter
7. **Integration test** — Full request against Anthropic API with cch hash
8. **max_tokens / Stainless** — Research and add if needed

## AnthropicOAuth Target State (Opus 4.6)

OAuth requests for Opus 4.6 share most of the same body params as fast mode.
The `AnthropicOAuth` provider will need the same `context_management`,
`metadata`, billing header, and cch serializer — just without the
`research_preview_2026_02` field and fast mode beta header.

This means `AnthropicOAuth` also needs the seed, salt, and identity fields.
Consider a shared helper or base struct for the common OAuth+Opus4.6 config.

## AnthropicFastMode Target State

```rust
pub struct AnthropicFastMode {
    pub access_token: String,
    pub cch_seed: u64,
    pub cc_version_salt: String,
    pub user_id: String,       // persisted identity
    pub account_uuid: String,  // from OAuth profile
    pub session_id: String,    // per-session UUID v4
}

impl RequestCustomizer for AnthropicFastMode {
    fn apply_chat_options(&self, options: ChatOptions, _thinking_budget: u32) -> ChatOptions {
        let beta = format!("{},{}", OPUS_46_BETA, FAST_MODE_BETA);
        let metadata_user_id = build_user_id(&self.user_id, &self.account_uuid, &self.session_id);

        options
            .with_extra_headers(oauth_headers(&self.access_token, &beta))
            .with_capture_reasoning_content(true)
            .with_reasoning_effort(ReasoningEffort::Adaptive)
            .with_extra_body(json!({
                "research_preview_2026_02": "active",
                "context_management": {
                    "edits": [{"type": "clear_thinking_20251015", "keep": "all"}]
                },
                "metadata": {
                    "user_id": metadata_user_id
                }
            }))
            .with_body_serializer(CchSerializer { seed: self.cch_seed })
    }

    fn apply_chat_request(&self, request: ChatRequest) -> ChatRequest {
        let first_user_msg = first_user_message_text(&request);
        let billing = build_billing_header(&first_user_msg, &self.cc_version_salt, VERSION);
        let system = format!("{}\n\nYou are Claude Code, Anthropic's official CLI for Claude.", billing);
        request.with_system(system)
    }

    fn cooldown_on_rate_limit(&self) -> bool { true }
    fn is_fast_mode(&self) -> bool { true }
}
```

## File Reference

| File | What |
|------|------|
| `src/llm/provider/mod.rs` | `RequestCustomizer` trait |
| `src/llm/provider/anthropic.rs` | All 3 Anthropic providers + hash functions |
| `src/llm/provider/openrouter.rs` | OpenRouter provider |
| `src/llm/agent.rs` | `resolve_provider()`, `exec_chat_with_retry()` |
| `src/config.rs` | `AgentRuntimeConfig` — needs `cch_seed`, `cc_version_salt` |
| `src/auth.rs` | `OAuthCredentials` — needs `account_uuid` |
| `~/Work/rust-genai` (codey-dev) | `BodySerializer` trait, `ReasoningEffort::Adaptive` |
