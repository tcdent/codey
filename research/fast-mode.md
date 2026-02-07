# Anthropic Fast Mode (Research Preview)

Research findings on Anthropic's "fast mode" API feature, reverse-engineered from the Claude Code CLI source (`@anthropic-ai/claude-code@2.1.37`).

## Overview

Fast mode delivers faster Opus 4.6 responses at a higher cost per token. It is not a different model - it uses the same Opus 4.6 with a different API configuration that prioritizes speed over cost efficiency. Internally code-named "penguins" (feature flag: `tengu_penguins_enabled`).

## API Configuration

### Beta Header

Fast mode is activated by appending a beta string to the `anthropic-beta` header:

```
anthropic-beta: research-preview-2026-02-01
```

In the Claude Code source, when fast mode conditions are met:

```js
var opA = "research-preview-2026-02-01";

// When building the request, if fast mode is active:
if (fastModeActive) {
    betas.push(opA);  // appended to the anthropic-beta header
}
```

### Model Restriction

Fast mode only works on models containing `opus-4-6` in their model ID:

```js
function X0(A) {
    if (!n4()) return false;
    let q = A ?? eO1();
    return i9(q).toLowerCase().includes("opus-4-6");
}
```

### Pricing (Per Million Tokens)

| Mode | Input | Output | Cache Write | Cache Read |
|------|-------|--------|-------------|------------|
| Standard Opus 4.6 | $5 | $25 | $6.25 | $0.50 |
| Standard Opus 4.6 (>200K) | $10 | $37.50 | $12.50 | $1.00 |
| **Fast mode** | **$30** | **$150** | **$37.50** | **$3.00** |
| **Fast mode (>200K)** | **$60** | **$225** | **$75** | **$6.00** |

Fast mode pricing is 6x input / 6x output compared to standard.

## Rate Limiting & Cooldown

Fast mode has separate rate limits from standard Opus 4.6. Claude Code implements a cooldown mechanism:

### Cooldown Constants

```js
j79 = 1800000   // 1,800,000 ms = 30 minutes (default/server-provided cooldown)
M79 = 20000     // 20,000 ms = 20 seconds (minimum cooldown)
W79 = 600000    // 600,000 ms = 10 minutes (floor cooldown)
```

### Cooldown Trigger

On 429 (rate limit) or overloaded responses:

```js
// Cooldown duration: max of (server retry-after OR 30min default, 10min floor)
let cooldownDuration = Math.max(retryAfterMs ?? j79, W79);
triggerCooldown(Date.now() + cooldownDuration);
```

During cooldown:
1. Fast mode automatically falls back to standard Opus 4.6
2. The fast mode header is omitted from requests
3. When cooldown expires, fast mode automatically re-enables

### Overage/Billing Rejection

The API returns an `anthropic-ratelimit-unified-overage-disabled-reason` response header when extra usage credits are exhausted. Possible values:

- `out_of_credits` - Extra usage credits exhausted
- `org_level_disabled` / `org_service_level_disabled` - Org disabled extra usage
- `member_level_disabled` - Account-level disable
- `overage_not_provisioned` / `no_limits_configured` - Extra usage not set up

On receiving this header, fast mode is permanently disabled for the session (not just a cooldown).

### 400 Error Handling

If the API returns HTTP 400 with message "Fast mode is not enabled", fast mode is disabled for the session.

## Org-Level Check

Before enabling fast mode, Claude Code checks an org endpoint:

```
GET {BASE_API_URL}/api/claude_code_penguin_mode
Authorization: Bearer {access_token}  (or x-api-key)
anthropic-beta: {standard_beta_header}
```

Response includes `enabled` (boolean) and `disabled_reason` (string) fields. Possible `disabled_reason` values: `"free"`, `"preference"`, `"extra_usage_disabled"`.

## Implementation Plan for Codey

### Config

Add `fast_mode` boolean to `[agents.foreground]` config:

```toml
[agents.foreground]
model = "claude-opus-4-6"
fast_mode = true
```

### Header Construction

When `fast_mode` is enabled and model contains `opus-4-6`, append `research-preview-2026-02-01` to the `anthropic-beta` header value.

### Cooldown Strategy

On rate limit (429) or overloaded error while fast mode is active:
1. Record cooldown start time
2. Strip the fast mode beta header from subsequent requests
3. After 20 minutes, automatically re-enable the header
4. Log the cooldown transition for visibility

### Error Detection

The genai library surfaces HTTP errors. We need to detect:
1. **429/overloaded with fast mode active** -> trigger cooldown
2. **400 "Fast mode is not enabled"** -> disable fast mode permanently for session

## References

- [Claude Code Fast Mode docs](https://code.claude.com/docs/en/fast-mode)
- Source: `@anthropic-ai/claude-code@2.1.37` (`cli.js`, minified)
- Feature flag: `tengu_penguins_enabled`
- Beta header: `research-preview-2026-02-01`
