# genai BodySerializer Extension

Added in genai commit `1b35d12` on the `codey-dev` branch (`~/Work/rust-genai`).

## Problem

Fast mode requires computing a `cch` hash over the fully serialized JSON request
body. genai handles serialization internally (`reqwest_builder.json(&content)` in
`web_client.rs`), so there's no hook to post-process the body between
serialization and send. We needed a way for downstream code (codey) to control
the final body bytes without forking the entire request pipeline.

## Solution: BodySerializer trait

A trait that replaces the default `serde_json::to_string` serialization.
Implementations receive the fully constructed `serde_json::Value` payload and
return the final HTTP body string.

```rust
pub trait BodySerializer: Send + Sync {
    fn serialize(&self, payload: &Value) -> Result<String, String>;
}
```

Wrapped in `DynBodySerializer` (an `Arc<dyn BodySerializer>`) for ergonomic
storage in `ChatOptions` — provides `Clone` and manual `Debug` so it coexists
with serde derives via `#[serde(skip)]`.

## genai file map

### Trait + wrapper definition
- **`src/chat/chat_options.rs:601-655`** — `BodySerializer` trait, `DynBodySerializer` struct

### ChatOptions integration
- **`src/chat/chat_options.rs:78-82`** — `body_serializer` field on `ChatOptions` (`#[serde(skip)]`)
- **`src/chat/chat_options.rs:192-196`** — `with_body_serializer()` builder method
- **`src/chat/chat_options.rs:581-585`** — `body_serializer()` accessor on `ChatOptionsSet`

### Serialization hook point
- **`src/webc/web_client.rs:64-97`** — `new_req_builder()` accepts `Option<&DynBodySerializer>`.
  When present: calls `serializer.serialize(&content)`, sets `content-type: application/json` +
  raw body. When `None`: uses the existing `.json(&content)` path unchanged.
- **`src/webc/web_client.rs:48-57`** — `do_post()` passes serializer through to `new_req_builder()`
- **`src/webc/error.rs:24`** — `BodySerialization(String)` error variant

### Call sites in client
- **`src/client/client_impl.rs:88`** — `exec_chat` → `do_post(..., options_set.body_serializer())`
- **`src/client/client_impl.rs:140`** — `exec_chat_stream` → `new_req_builder(..., options_set.body_serializer())`
- **`src/client/client_impl.rs:193`** — `exec_embed` → `do_post(..., None)` (embeddings don't use custom serialization)

## Other changes in same commit

- **Adaptive reasoning** — `ReasoningEffort::Adaptive` variant added (`chat_options.rs:209`),
  handled in Anthropic adapter (`adapter_impl.rs:23-32`) to emit `{"thinking": {"type": "adaptive"}}`.
  Gemini adapter returns `None` for adaptive (`gemini/adapter_impl.rs:44`).
- **`?beta=true` commented out** — `adapter_impl.rs:119-121`. Claude Code appends this to the
  messages URL for OAuth requests. Unclear if it gates anything server-side; left as TODO.

## How codey will use this

The integration point is in `src/llm/agent.rs:590-609` (`exec_chat_with_retry`),
where `ChatOptions` is built with `with_extra_headers(headers)`. For fast mode
requests, the agent will also call `.with_body_serializer(...)`.

### Data flow

```
codey agent.rs                           genai
─────────────                            ─────
1. build_billing_header()                
   → "...cch=00000;..."                  
2. inject into system prompt blocks      
3. set extra_body with fast mode fields  
4. construct CchSerializer { seed }      
5. ChatOptions::with_body_serializer()   
                                         6. to_web_request_data() builds Value
                                         7. new_req_builder() calls serializer
                                            → serialize(payload: &Value)
                                              a. serde_json::to_string
                                              b. compute_cch (xxHash64)
                                              c. replace "cch=00000" → "cch=<hash>"
                                            → returns final body string
                                         8. reqwest sends with raw body
```

### codey files involved

- **`src/llm/anthropic.rs:158-218`** — `compute_cch()`, `apply_cch()`, `build_billing_header()`,
  `compute_cc_version_suffix()`. All pure functions, ready to use.
- **`src/llm/agent.rs:502-545`** — `resolve_provider()` already returns `ApiProvider::Anthropic(FastMode{..})`
  when eligible. `is_fast_mode_eligible()` checks config + cooldown timer.
- **`src/llm/agent.rs:590-609`** — Where `ChatOptions` is assembled. This is where
  `.with_body_serializer()` and `.with_extra_body()` calls need to be added for fast mode.
- **`src/config.rs`** — Needs `cch_seed: u64` and `cc_version_salt: String` fields
  (sensitive, not hardcoded in source).

### Remaining genai work

- **`extra_body`** already works (`adapter_impl.rs:303-310`) — merges top-level keys into payload.
  Fast mode fields (`research_preview_2026_02`, `context_management`, `metadata`) go here.
- **System prompt injection** — genai's Anthropic adapter already prepends a Claude Code
  identifier block for OAuth (`adapter_impl.rs:192-222`). The billing header needs to be
  the *first* block. This may require adjusting the prepend logic or passing the billing
  header text through a new ChatOptions field so the adapter can position it correctly.
  Alternatively, codey can inject it as the first system message in the ChatRequest before
  handing off to genai.
