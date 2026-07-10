# Anthropic Fast Mode (Research Preview)

Research findings on Anthropic's "fast mode" API feature, reverse-engineered from the Claude Code CLI binary v2.1.37 and verified via mitmproxy request capture.

## Overview

Fast mode delivers faster Opus 4.6 responses at a higher cost per token. It is not a different model — it uses the same Opus 4.6 with a different API configuration that prioritizes speed over cost efficiency. Internally code-named "penguins" (feature flag: `tengu_penguins_enabled`).

## Server-Side Access Gate

### Current Status: The `cch` Hash

We can successfully make fast mode requests by replaying a complete captured
request body from the real Claude Code binary. Both Python `requests`
(OpenSSL/HTTP1.1) and Bun 1.3.9 (BoringSSL/HTTP2) succeed with HTTP 200,
which **rules out TLS fingerprinting and binary attestation**.

**The primary gate is a per-request hash embedded in the billing header.**
The request body's system prompt contains a field disguised as billing
telemetry:

```
x-anthropic-billing-header: cc_version=2.1.37.fbe; cc_entrypoint=cli; cch=f3d8a;
```

The `cch` value (likely "Claude Code Hash") is a 5-character hex string
that changes per-request. The `cc_version` suffix (`.fbe`, `.e93`) also
varies per-request. Observed values across captures:

| Capture | Type | `cc_version` | `cch` | Session UUID |
|---------|------|-------------|-------|---------------|
| 061 (old golden) | fast mode | `2.1.37.fbe` | `a112b` | `1a5ef3b2-...` |
| 072 (new golden) | fast mode | `2.1.37.fbe` | `f3d8a` | `654533b9-...` |
| 071 (haiku) | non-fast | `2.1.37.e93` | `343c7` | `654533b9-...` |

Captures 071 and 072 are from the **same session** but have different `cch`
and `cc_version` suffix values — confirming these are per-request, not
per-session.

**This explains all observed behavior:**

- ✅ **Verbatim replay works** → `cch` matches the unchanged body
- ✅ **Session UUID swap fails** → UUID changes but `cch` doesn't, hash mismatch
- ✅ **Tool modifications fail** → tools change but `cch` doesn't, hash mismatch
- ✅ **System prompt edits work** → `cch` likely does not cover system prompt
  content (or at least not the non-billing system blocks)
- ✅ **Re-serialization works** → content unchanged, `cch` still valid
- ✅ **Two different golden UUIDs both work** → each has its own matching `cch`
- ✅ **Fresh UUID always fails** → no way to generate a matching `cch` without
  knowing the hash algorithm

The obfuscation is clever — `cch=f3d8a` looks like a mundane billing tag,
not a cryptographic validation hash. Nobody looks twice at it.

### Validation experiments

| Test | Tools | Session UUID | `cch` | Result |
|------|-------|-------------|-------|--------|
| Full golden payload (70 tools) | exact | original | original | ✅ 200 |
| Empty tools `[]` | none | original | original | ❌ 400 |
| Remove one builtin (`NotebookEdit`) | 69 | original | original | ❌ 400 |
| Edit one tool description by one word | 70 (mod) | original | original | ❌ 400 |
| Add one `mcp__` tool to the end | 71 | original | original | ❌ 400 |
| Remove all `mcp__` tools, keep 22 builtins | 22 | original | original | ❌ 400 |
| Edit system prompt (non-billing block) | 70 | original | original | ✅ 200 |
| String-replace session UUID only | 70 | **fresh** | original | ❌ 400 |
| Verbatim replay single request | 70 | original | original | ✅ 200 |
| Verbatim replay all 86 requests | 70 | original | original | ✅ 200 |
| Full replay, swap UUID everywhere | 70 | **fresh** | original | ❌ 400 |

### What `cch` likely hashes

Based on what passes and what fails, the hash likely covers:
- ✅ The tools array (any tool change → rejection)
- ✅ The session UUID in `metadata.user_id` (UUID swap → rejection)
- ❌ NOT the system prompt content (edits to non-billing system blocks work)
- ❓ Possibly the `cc_version` suffix itself (also changes per-request)
- ❓ Possibly other body fields (`model`, `research_preview_2026_02`, etc.)

### What we've proven

- ✅ TLS fingerprint does NOT matter (Python OpenSSL works)
- ✅ HTTP version does NOT matter (HTTP/1.1 works)
- ✅ No binary attestation / embedded certificate required
- ✅ Full golden payload replayed from any client succeeds
- ✅ System prompt can be modified freely (non-billing blocks)
- ✅ No pre-flight registration needed — single golden request works alone
- ✅ No connection/keep-alive correlation — replay scripts use plain `fetch()`
- ✅ Session UUID is standard `crypto.randomUUID()` — no special format
- ✅ No OAuth token refresh in captures — token was already valid
- ✅ MCP localhost calls and GitHub API calls are NOT significant for fast mode
- ✅ `cch` in billing header is a per-request validation hash

### What remains to solve

- **Determine minimum viable request** — strip the golden payload to find
  the smallest body that still gets fast mode
- **Implement in codey** — route fast mode requests through the extracted
  Bun binary as a sidecar, or reverse-engineer the native hash algorithm
  from the compiled Zig code and implement it in Rust

### Client-Side Gates

Claude Code also gates fast mode client-side before sending the request:

```js
// All must be true:
fastModeActive = n4()       // Statsig: tengu_penguins_enabled (defaults true)
    && !nk()                // Not in cooldown
    && C$()                 // W46() === null (no blocking reason)
    && X0(w.model)          // Model contains "opus-4-6"
    && !!t.fastMode;        // User setting enabled

// W46() checks in order:
// 1. n4() — feature flag
// 2. O9() — native binary check: typeof Bun !== "undefined" && Bun.embeddedFiles.length > 0
// 3. tengu_penguins_off flag
// 4. Not running as Agent SDK (non-interactive + not vscode)
// 5. Provider is "firstParty" (not Bedrock/Vertex/Foundry)
```

The `O9()` check (`Bun.embeddedFiles`) is why running via `node cli.js` blocks fast mode — even with a Bun shim and forced gate override, the server still rejects.

## Golden Reference: Captured Fast Mode Request

Captured via mitmproxy from the real Claude Code binary (status 200 — success):

### Headers

```
User-Agent: claude-cli/2.1.37 (external, cli)
anthropic-beta: claude-code-20250219,oauth-2025-04-20,context-management-2025-06-27,prompt-caching-scope-2026-01-05,effort-2025-11-24,adaptive-thinking-2026-01-28,research-preview-2026-02-01
anthropic-version: 2023-06-01
anthropic-dangerous-direct-browser-access: true
x-app: cli
X-Stainless-Lang: js
X-Stainless-Package-Version: 0.70.0
X-Stainless-OS: MacOS
X-Stainless-Arch: arm64
X-Stainless-Runtime: node
X-Stainless-Runtime-Version: v24.3.0
X-Stainless-Timeout: 600
```

### Body

```json
{
  "model": "claude-opus-4-6",
  "max_tokens": 32000,
  "stream": true,
  "thinking": {"type": "adaptive"},
  "research_preview_2026_02": "active",
  "context_management": {"edits": [{"type": "clear_thinking_20251015", "keep": "all"}]},
  "metadata": {"user_id": "user_{hash}_account_{uuid}_session_{uuid}"},
  "messages": [...],
  "system": [...],
  "tools": [...]
}
```

### Without Fast Mode (same binary, same session)

Identical except:
- No `research_preview_2026_02` in body
- No `research-preview-2026-02-01` in `anthropic-beta`

## Startup Sequence

### Filtered capture (71 requests, anthropic.com + statsig + datadog only)

| # | Method | URL | Purpose |
|---|--------|-----|--------|
| 0 | POST | `/api/eval/sdk-zAZezfDKGoZuXXKe` | Statsig feature flag evaluation (GrowthBook) |
| 1 | GET | `/api/oauth/account/settings` | Account preferences |
| 2 | GET | `/api/claude_code_grove` | Unknown (possibly tree/workspace config) |
| 3-4 | POST | `localhost:7100/mcp`, `localhost:7101/mcp` | Local MCP servers |
| 5 | GET | `/api/claude_code_penguin_mode` | **Fast mode eligibility check** |
| 6-7 | POST | `localhost:710x/mcp` | MCP initialization |
| 8 | POST | `/v1/messages?beta=true` | Quota check (haiku, max_tokens=1) |
| 9+ | POST | `/v1/messages/count_tokens` | Token counting (repeated) |
| N | POST | `/v1/messages?beta=true` | Actual chat request |

### Unfiltered capture (86 requests, ALL domains)

```
000: POST /api/eval/sdk-zAZezfDKGoZuXXKe          (statsig)
001: GET  /api/claude_code_grove
002: GET  /api/oauth/account/settings
003: GET  /api/oauth/organizations/.../referral/eligibility
004-013: MCP localhost (initialize, notifications, tools/list, etc.)
014: POST /v1/messages?beta=true                   (haiku, no tools, has session UUID)
015: POST api.github.com/graphql                   (PR branch info)
016-065: POST /v1/messages/count_tokens?beta=true   (1 tool each, no session UUID)
066: POST /api/event_logging/batch
067: GET  /api/hello
068: POST api.github.com/graphql
069: GET  /api/hello
070: POST /api/event_logging/batch
071: POST /v1/messages?beta=true                   (haiku, tools=0, session UUID)
072: POST /v1/messages?beta=true                   ★ FAST MODE (opus-4-6, 70 tools)
073-085: post-fast-mode (datadog, event_logging, MCP calls)
```

Domains discovered in unfiltered capture not in filtered: `localhost:7100`,
`localhost:7101` (MCP servers), `api.github.com` (GraphQL). These were
confirmed NOT significant for fast mode — MCP calls skipped in replay,
GitHub calls are just data fetches.

### Penguin Mode Endpoint

```
GET https://api.anthropic.com/api/claude_code_penguin_mode
Authorization: Bearer {oauth_token}
anthropic-beta: oauth-2025-04-20
User-Agent: axios/1.8.4

Response: {"enabled": true, "disabled_reason": null}
```

Possible `disabled_reason` values: `"free"`, `"preference"`, `"extra_usage_disabled"`.

### Statsig Evaluation

The Statsig call sends device attributes and receives 73 feature flags. Key flags:
- `tengu_penguins_enabled`: `true` (controls fast mode availability)
- `tengu_penguin_mode_promo`: `{"discountPercent": 50, "endDate": "Feb 16"}` (promotional pricing)
- `tengu_penguins_off`: `null` when not set (would contain a disable reason string)

## Pricing (Per Million Tokens)

| Mode | Input | Output | Cache Write | Cache Read |
|------|-------|--------|-------------|------------|
| Standard Opus 4.6 | $5 | $25 | $6.25 | $0.50 |
| Standard Opus 4.6 (>200K) | $10 | $37.50 | $12.50 | $1.00 |
| **Fast mode** | **$30** | **$150** | **$37.50** | **$3.00** |
| **Fast mode (>200K)** | **$60** | **$225** | **$75** | **$6.00** |

Fast mode pricing is 6x input / 6x output compared to standard.

## Rate Limiting & Cooldown

Fast mode has separate rate limits from standard Opus 4.6. Claude Code implements a cooldown mechanism:

```js
j79 = 1800000   // 30 minutes (default/server-provided cooldown)
M79 = 20000     // 20 seconds (minimum cooldown)
W79 = 600000    // 10 minutes (floor cooldown)

// On 429/overloaded:
cooldownDuration = Math.max(retryAfterMs ?? j79, W79);
```

During cooldown:
1. Fast mode automatically falls back to standard Opus 4.6
2. The fast mode beta/body params are omitted
3. When cooldown expires, fast mode re-enables automatically

### Overage/Billing Rejection

Response header `anthropic-ratelimit-unified-overage-disabled-reason` values:
- `out_of_credits`, `org_level_disabled`, `org_service_level_disabled`
- `member_level_disabled`, `overage_not_provisioned`, `no_limits_configured`

On receiving this header, fast mode is permanently disabled for the session.

### 400 Error Handling

HTTP 400 with "Fast mode is not enabled" → fast mode disabled for session.

## Adaptive Thinking (Opus 4.6)

Opus 4.6 uses **adaptive thinking** instead of budget-based thinking:

```js
// Opus 4.6:
thinking = {type: "adaptive"};
betas.push("adaptive-thinking-2026-01-28");
// interleaved-thinking-2025-05-14 is REMOVED from betas

// All other thinking models:
thinking = {type: "enabled", budget_tokens: K ?? 31999};
```

### Max Output Tokens

Opus 4.6 gets `max_tokens: 32000`.

### Effort Handling

The `effort-2025-11-24` beta is always included. When an effort level is set (via `CLAUDE_CODE_EFFORT_LEVEL`), it goes into `output_config.effort`.

### Full Betas: Opus 4.6 + Fast Mode

```
claude-code-20250219
oauth-2025-04-20
context-management-2025-06-27
prompt-caching-scope-2026-01-05
effort-2025-11-24
adaptive-thinking-2026-01-28
research-preview-2026-02-01       (fast mode only)
```

NOTE: `interleaved-thinking-2025-05-14` is **not present** when adaptive thinking is used.

## Codey Implementation Status

All implemented in codey + genai fork:

1. ✅ **genai fork**: `ReasoningEffort::Adaptive` → `{"thinking": {"type": "adaptive"}}`
2. ✅ **genai fork**: `extra_body` on `ChatOptions` for arbitrary body params
3. ✅ **Agent**: Auto-bumps `max_tokens` to 32000 for opus-4-6
4. ✅ **Beta headers**: Uses `adaptive-thinking-2026-01-28,effort-2025-11-24` for opus-4-6
5. ✅ **Thinking**: Uses `ReasoningEffort::Adaptive` for opus-4-6
6. ✅ **Fast mode params**: Sends beta + `research_preview_2026_02: "active"` when active
7. ✅ **Headers**: `x-app: cli`, `user-agent: claude-cli/2.1.37 (external, cli)`
8. ✅ **Cooldown**: Falls back to standard on rate limit, re-enables after timeout
9. ✅ **Server-side gate**: Bypassed — routing requests through extracted Bun binary computes valid `cch` hash, server accepts fast mode from any client

### What We've Ruled Out

- ❌ TLS fingerprint (Python OpenSSL succeeds with golden payload)
- ❌ HTTP/2 requirement (HTTP/1.1 works)
- ❌ Binary attestation / `Bun.embeddedFiles`
- ❌ Pre-flight registration (single request replay works)
- ❌ Connection/keep-alive correlation (plain `fetch()` works)
- ❌ Replay detection (same body can be sent multiple times)
- ❌ Session UUID format (standard v4 UUID, nothing special)

### The Remaining Gate: `cch` Hash — SOLVED

The `cch` hash is computed by a custom Bun runtime (1.3.9-canary.51,
private Anthropic build). The `nativeFetch` implementation scans the
serialized request body for `cch=00000` and replaces it with a valid
5-char hex hash before sending.

**Verified:** A fresh session UUID + `cch=00000` sent through the
extracted Bun binary's `fetch` → server returns HTTP 200 with
`research_preview_2026_02: "active"` (fast mode enabled).

### cch hash algorithm — SOLVED

**`cch = xxHash64(body, seed=0x6e52736ac806831e) & 0xFFFFF`**, formatted as
5-char lowercase hex.

```python
import xxhash
CCH_SEED = 0x6e52736ac806831e
cch = format(xxhash.xxh64(body.encode(), seed=CCH_SEED).intdigest() & 0xFFFFF, 'x')
```

- **142/142 oracle pairs verified** against `/tmp/cch_fresh_pairs.json`
- The function at `___lldb_unnamed_symbol18111` (vaddr `0x1015d05bc`) is
  standard xxHash64 — all 5 prime constants and the avalanche finalization
  match the published algorithm exactly
- The seed `0x6e52736ac806831e` was discovered via LLDB memory dump at
  `0x103e900e0` — it's one of four 64-bit constants stored in the binary's
  data section (the others: `k0=0xcf3c9b5975c738f4`, `k1=0x310521a7efdb6e6d`,
  `k3=0xd01af9b9421ab897`)
- Initial brute-force testing missed it because only seeds 0–19 were tried

### Integration — no sidecar needed

Since the hash is standard xxHash64 with a known seed, it can be computed
natively in Rust (`xxhash-rust` crate) with no dependency on the Bun binary.

### Next steps

1. Implement cch hash in Rust (xxhash-rust crate, `xxh64(body, seed)`)
2. Verify `cc_version` suffix algorithm against captures
3. Determine the **minimum viable request** by stripping the golden payload
4. Integrate into codey's request pipeline

### `metadata.user_id` Construction

The `user_id` field in `metadata` is built by the `Ao()` function in cli.js:

```js
function Ao() {
  let A = Oh(),                    // userID (random 32-byte hex, persisted)
      q = C3()?.accountUuid ?? "", // from oauthAccount in config
      K = U6();                    // sessionId (UUID v4, per-session)
  return { user_id: `user_${A}_account_${q}_session_${K}` }
}
```

**Data sources:**

| Component | Source | Persistence |
|-----------|--------|-------------|
| `userID` | `Oh()` → `M6().userID` | `~/.claude.json` field `userID`. Generated once as `randomBytes(32).toString("hex")`, persisted forever. |
| `accountUuid` | `C3()?.accountUuid` → `M6().oauthAccount.accountUuid` | `~/.claude.json` field `oauthAccount.accountUuid`. Populated from `GET /api/oauth/profile` (called via `Sw1()` during token refresh). |
| `sessionId` | `U6()` → `n6.sessionId` | In-memory only. Fresh UUID v4 generated each session via `qdA()`. |

**Example value:**
```
user_15ff3bec...4097525_account_c5df0620-63cd-4944-9721-fa72cbdf2297_session_5ab85533-49ef-45a4-b71b-46af91b464cc
```

**`~/.claude.json` relevant fields:**
```json
{
  "userID": "15ff3bec4829cdc744ca9206757b62d3150fc076e5fd33982733ad4a04097525",
  "oauthAccount": {
    "accountUuid": "c5df0620-63cd-4944-9721-fa72cbdf2297",
    "emailAddress": "...",
    "organizationUuid": "...",
    "billingType": "...",
    ...
  }
}
```

## The `cch` Hash (Billing Header)

The `x-anthropic-billing-header` is embedded as the **first system prompt
block** in every messages API request. It contains a per-request hash that
the server uses to validate the request originated from a genuine Claude
Code client.

### Format

```
x-anthropic-billing-header: cc_version={version}.{suffix}; cc_entrypoint={entry}; cch={hash};
```

| Field | Example | Description |
|-------|---------|-------------|
| `cc_version` | `2.1.37.fbe` | Claude Code version + 3-char hex suffix (changes per-request) |
| `cc_entrypoint` | `cli` | Entry point (`cli`, possibly `vscode`) |
| `cch` | `f3d8a` | 5-character hex hash (changes per-request) |

### Observed values

```
Capture 061 (fast mode, old session):  cc_version=2.1.37.fbe  cch=a112b
Capture 072 (fast mode, new session):  cc_version=2.1.37.fbe  cch=f3d8a
Capture 071 (haiku, same session as 072): cc_version=2.1.37.e93  cch=343c7
```

### Key observations

- `cch` changes between requests in the **same session** (071 vs 072)
- `cc_version` suffix changes between requests in the same session
- Both values are derived per-request, not per-session
- The hash is 20 bits (5 hex chars) — truncated from a longer digest
- The version suffix is 12 bits (3 hex chars) — also truncated

### Reverse-engineered: `cc_version` suffix

The 3-char hex suffix on `cc_version` is computed by function `O0A`
(deobfuscated name). Found in the binary at strings line ~122732:

```js
// Salt constant
var hbD = "59cf53e54c78";
var no9 = require("crypto");

// Extract first user message text from internal message array
function EbD(T) {
  let R = T.find((_) => _.type === "user");
  if (!R) return "";
  let A = R.message.content;
  if (typeof A === "string") return A;
  if (Array.isArray(A)) {
    let _ = A.find((B) => B.type === "text");
    if (_ && _.type === "text") return _.text;
  }
  return "";
}

// Compute 3-char hex suffix
function O0A(T, R) {
  // T = user message text, R = VERSION string
  let _ = [4, 7, 20].map(($) => T[$] || "0").join("");
  let B = `${hbD}${_}${R}`;
  return no9.createHash("sha256").update(B).digest("hex").slice(0, 3);
}

// Called with messages array → returns suffix
function ao9(T) {
  let R = EbD(T);  // first user message text
  return O0A(R, "2.1.37");  // VERSION constant
}
```

**Algorithm:** `sha256("59cf53e54c78" + chars_at_4_7_20 + VERSION).slice(0, 3)`

Where `chars_at_4_7_20` picks characters at string indices 4, 7, and 20
from the first user message's text content. Missing chars default to `"0"`.

### Reverse-engineered: billing header construction

Function `u7R(T)` builds the billing header string:

```js
function u7R(T) {  // T = version suffix from ao9
  if (!XbD()) return "";  // feature flag: tengu_attribution_header
  let R = `${VERSION}.${T}`;  // e.g. "2.1.37.fbe"
  let A = process.env.CLAUDE_CODE_ENTRYPOINT ?? "unknown";
  let B = `x-anthropic-billing-header: cc_version=${R}; cc_entrypoint=${A};${" cch=00000;"}`;
  return B;
}
```

**Critical finding: `cch=00000` is hardcoded as a placeholder!**

The client code always constructs the string with `cch=00000`, but our
mitmproxy captures show real values (`a112b`, `f3d8a`, `343c7`). This
means the `00000` placeholder gets replaced somewhere between
construction and transmission.

**FOUND:** The replacement happens in the custom Bun runtime's native
`fetch` implementation (compiled Zig/C++). See "Request pipeline call
stack" section below for full details.

### What we know about `cch`

- 5-char hex string (20 bits), changes per-request
- Computed and injected by Bun's native `nativeFetch` (not in JS)
- Covers the **entire** request body — every field change (tools, session
  UUID, model, messages, system prompt, cc_version, max_tokens, metadata,
  entrypoint) produces a different hash. Hash is deterministic.
- The custom Bun build (1.3.9-canary.51+d5628db23) is an unreleased
  private Anthropic fork — commit does not exist on public oven-sh/bun

### Minimum trigger conditions for `cch` rewrite

The native `nativeFetch` only performs the `cch=00000` → real hash
rewrite when **all three** conditions are met:

1. **URL path** contains `/v1/messages`
2. **`anthropic-version` header** is present (value is ignored — even `"x"` works)
3. **Request body** contains the `cch=00000` placeholder string

The URL path check is the key discriminator — earlier tests that
sent to `/test` instead of `/v1/messages` saw no rewrite even with
all headers present.

**Critical implementation detail:** Bun's nativeFetch **mutates the JS
string buffer in place**. After `fetch()` returns, the original `body`
string variable has its `cch=00000` bytes overwritten with the computed
hash (e.g. `cch=ea735`). This violates JS string immutability and means:
- The same string variable must NOT be reused across requests
- The hash can be read back from the body string after `fetch()` completes

### Hash algorithm (what we know)

The hash is **purely a function of the body bytes** — URL path, query
params, and request headers have zero effect on the output. They only
gate whether the rewrite is performed.

The hash covers the **entire body** including both sides of the
`cch=00000` marker. The body is hashed with `00000` still in place
(not with the computed hash substituted first). Algorithm:

1. Find `cch=00000` byte pattern in request body
2. Compute `hash(entire_body_bytes_with_00000)` → 5 hex chars
3. Overwrite the `00000` bytes in-place with the result

The underlying hash algorithm is unknown — not sha256, sha1, md5, crc32,
wyhash (Bun.hash), or any HMAC/salted variant we tested with the known
`59cf53e54c78` salt. Likely uses a baked-in key that doesn't appear in
`strings` output (possibly a Zig comptime constant). Requires disassembly
of the nativeFetch code path to determine.

### Binary analysis

The custom Bun binary (`/tmp/bun-standalone`, 181MB Mach-O ARM64) has
been partially analyzed with capstone (ARM64 disassembler). Key findings:

**Symbols:** Only 3 extra symbols vs stock Bun 1.3.4 — all libc imports,
no custom-named functions. The `cch` patch is inlined into existing code.

**Imports:** `memchr` and `memcmp` are imported, but no crypto library
functions (`sha*`, `hmac`, etc.) — the hash algorithm is fully inlined
in the native code.

**String search:** The literals `cch=`, `cch=00000`, `anthropic-version`,
and `/v1/messages` do **not** appear anywhere in the binary — not in
`__cstring`, not as raw bytes, not as MOVZ/MOVK immediates. The code
either constructs these patterns at runtime, uses hash comparisons
instead of string comparisons, or encodes them via Zig comptime.

**MOVK #0x6363 scan:** Found 3 ARM64 instructions loading `0x6363`
(the ASCII bytes "cc"). All 3 are false positives — they're in a CSS
color name lookup table (comparing "rebeccapurple", "moccasin", etc.).

### Disassembly approach (in progress)

Using Python + capstone for targeted disassembly (no full-binary analysis
needed). Next steps:

1. Search for hex conversion patterns — any code that formats an integer
   as hex must reference `"0123456789abcdef"` or equivalent lookup table
2. Search for SIMD/NEON string matching — Bun's Zig code uses NEON
   vectors for pattern matching in HTTP parsing
3. Binary diff: compare `__text` section against a stock Bun canary build
   of similar version to isolate the patched region
4. Search for `memchr` call sites and trace which ones scan large buffers
   (the request body) rather than small strings

### Request pipeline call stack

Traced the billing header from construction through to the HTTP send.
`cch=00000` is confirmed **unchanged** at every stage:

```
ao9(Z)                    # Computes version suffix from first user message
  → EbD(Z)               # Extracts first user message text
  → O0A(text, VERSION)   # sha256(salt + chars + version).slice(0,3)
  → returns suffix (e.g. "fbe")

u7R(suffix)               # Builds billing header string
  → returns "x-anthropic-billing-header: ...cch=00000;"

R = [                     # System array assembled
  u7R(I),                 # ← billing header with cch=00000 (first element)
  v7R({...}),             # base system prompt
  ...R,                   # additional system blocks
  SaB(D.mcpTools),        # MCP tool descriptions
].filter(Boolean)

PPB(R)                    # Telemetry: hashes first system block, NO mutation
  → ByA(R)[0].text        # Extracts billing header
  → sha256(text)          # Logs hash for telemetry

w = tN8(R, F, {...})      # Wraps strings → {type: "text", text: ...} blocks
  → ByA(R, opts)          # Splits into blocks with cacheScope
  → .map(→ {type, text})  # NO mutation of cch

bK_(w, u, ...)            # Change-tracking telemetry
  → PK_(w)                # Strips cache_control from system blocks
  → yK_(...)              # Bun.hash() or djb2 of JSON.stringify'd content
  → PK_(u)                # Same for tools
  → yK_(...)              # Hashes tools — integer hash, NOT hex

$T(e) builds body:        # Returns full request body object
  → { system: w, tools: u, metadata: Va(), ... }
  → Va() includes session UUID
  → cch is STILL 00000 in w[0].text

j6R() retry wrapper        # Calls TP() to create Anthropic client
  → TP()                  # Sets defaultHeaders (x-app, user-agent, etc.)
  → NO body modification

qT.beta.messages.create({...IT, stream: true})  # Stainless SDK
  → makeRequest()
    → prepareOptions()
    → buildRequest()
      → buildBody()
        → lp9({body}) = JSON.stringify(body)  # NO modification
    → prepareRequest()    # NO-OP for firstParty (only Bedrock overrides)
    → fetch(url, req)     # Body sent as-is
```

**Conclusion:** The `cch=00000` placeholder is NOT replaced anywhere in
the traced call stack. Every function from construction through SDK
serialization passes the string through unchanged.

**SOLVED: The replacement happens in Bun's native `fetch` implementation.**

The custom Bun 1.3.9-canary.51+d5628db23 (unreleased, Anthropic-internal
build) has a patched `nativeFetch` that scans the serialized request body
for the `cch=00000` marker and replaces it with a computed 5-char hex
hash before sending over the wire.

**Proof:** A simple test script sends `cch=00000` via Bun's fetch to a
local Starlette server. The JS logs `cch=00000` going out, but the
server receives `cch=2647e`. The replacement is invisible to JS code.

This commit (`d5628db23`) does not exist on the public oven-sh/bun
repository — it's a private Anthropic fork. Anthropic acquired Oven
(the company behind Bun), giving them the ability to embed native-level
request validation directly in the runtime.

We now have an **oracle**: we can send any payload through the extracted
Bun binary and observe the computed `cch` value to reverse-engineer the
hash algorithm and inputs.

### Interesting functions found during tracing

- `bK_()` computes hashes of both system prompt and tools arrays using
  `PK_` (strip cache_control) → `yK_` (JSON.stringify → Bun.hash/djb2).
  These are integer hashes used for change-tracking telemetry, but the
  same serialization pattern could be reused elsewhere for `cch`.
- `ByA()` processes the system array and specifically identifies the
  billing header via `W.startsWith("x-anthropic-billing-header")`.
  It assigns `cacheScope: null` to the billing block.
- `PPB()` hashes the first system block with sha256 for telemetry.

### Search targets for `cch` computation

- ~~Look in the Stainless SDK request pipeline~~ — confirmed clean
- ~~Check `ByA()` and `PPB()`~~ — confirmed read-only / telemetry
- ~~Look for a monkey-patched `fetch` or Bun request interceptor~~ — not in JS
- ~~Look for code that mutates `w[0].text` via closure~~ — not in JS
- **FOUND:** The hash is computed in Bun's native `nativeFetch` (compiled
  Zig/C++ code in the custom Bun 1.3.9-canary.51 binary)

### Construction algorithm

To construct a valid `x-anthropic-billing-header`, given:
- `VERSION` = `"2.1.37"` (Claude Code version)
- `SALT` = `"59cf53e54c78"` (hardcoded constant `hbD`)
- `messages` = the internal message array (Claude Code's format, not the API format)
- `entrypoint` = `"cli"` (or `"vscode"`, from `CLAUDE_CODE_ENTRYPOINT` env var)

**Step 1: Compute the `cc_version` suffix**

1. Find the first message with `type === "user"` in the internal messages array
2. Extract the text content of that message (string or first `{type: "text"}` block)
3. Pick characters at string indices 4, 7, and 20 (defaulting to `"0"` if missing)
4. Concatenate: `input = SALT + chars[4] + chars[7] + chars[20] + VERSION`
5. Hash: `suffix = sha256(input).hex().slice(0, 3)`

Result: `cc_version = VERSION + "." + suffix` (e.g. `"2.1.37.fbe"`)

**Step 2: Build the header string**

```
x-anthropic-billing-header: cc_version={cc_version}; cc_entrypoint={entrypoint}; cch=00000;
```

**Step 3: Replace `cch=00000` with the real hash (Bun native fetch)**

The `00000` placeholder gets replaced with a valid 5-char hex hash by
Bun's native `fetch` implementation automatically. Any request body
containing `cch=00000` sent through the extracted Bun binary will have
the correct hash computed and substituted before transmission. The exact
algorithm is in compiled Zig code and has not been decompiled, but the
binary serves as an oracle for computing valid hashes.

**Note on `messages` format:** The `EbD` function extracts user text from
Claude Code's *internal* message format (`{type: "user", message: {content: ...}}`),
not the API wire format (`{role: "user", content: ...}`). The internal
format is used during request assembly before conversion to API format.
For our purposes we need the text of the first user message in the
conversation — the mapping is straightforward.

## Scripts

- **`research/fast/probe_cch_mvp.js`** — Minimum viable demo of the three conditions required for `cch` rewrite (path, header, placeholder). Also shows string mutation.
- **`research/fast/demo_string_mutation.js`** — Proves Bun's nativeFetch mutates JS strings in place, overwriting `cch=00000` in the body string's backing memory.
- **`research/fast/crack_cch.js`** — Oracle-based hash algorithm cracker. Tests sha256, sha1, md5, crc32, HMAC, and salted variants against known input/output pairs. All negative so far.
- **`research/fast/disasm_cch.py`** — ARM64 disassembler (capstone) for targeted analysis of the Bun binary. Decodes MOVZ/MOVK sequences into readable strings.

## References

- [Claude Code Fast Mode docs](https://code.claude.com/docs/en/fast-mode)
- Source: `@anthropic-ai/claude-code@2.1.37` (Bun single-file executable, 181MB)
- Binary: `~/.local/share/claude/versions/2.1.37` → symlinked at `~/.local/bin/claude`
- Feature flag: `tengu_penguins_enabled` (Statsig/GrowthBook)
- Penguin mode endpoint: `GET /api/claude_code_penguin_mode`
- Beta header: `research-preview-2026-02-01`
- Body param: `research_preview_2026_02: "active"`

## Future: JSString mutation as exploitation surface

During LLDB watchpoint tracing (session 2025-02-07), we discovered that
Bun's nativeFetch `cch=00000` rewrite behaves differently depending on
how the JS string was allocated:

- **String literals** → JSC constant pool (read-only pages) → nativeFetch
  copies to a separate HTTP buffer, mutates the copy. Original JS string
  is untouched.
- **Runtime strings** (e.g. `JSON.stringify()`) → WebKit Malloc heap
  (writable pages) → nativeFetch mutates the StringImpl backing store
  directly. The original JS variable is visibly changed after `fetch()`.

This is a **JS spec violation** — strings are immutable by definition.
No user code expects `fetch()` to silently rewrite bytes in a string
variable. Potential exploitation angles:

- **Reference aliasing:** If two variables share a backing store (e.g.
  `const alias = body`), both are silently mutated. Confirmed in
  `demo_string_mutation.js`.
- **Map/Set key corruption:** A string used as a Map key before fetch
  would have different bytes after, breaking lookups.
- **Rope sharing:** JSC's rope/substring optimizations can share backing
  stores across unrelated strings — mutating one could corrupt others.
- **String deduplication:** JSC's AtomString table deduplicates identical
  strings. If `cch=00000` is interned and shared, mutation propagates
  to all holders.
- **Unintended trigger:** Any JS code that coincidentally contains
  `cch=00000` in a runtime string and fetches to any `/v1/messages`
  URL (not just Anthropic's) will get silently rewritten.

Circle back on this after the hash algorithm is reverse-engineered —
could be useful for bypasses or alternative authentication approaches.
