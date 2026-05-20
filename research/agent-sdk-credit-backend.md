# Agent backend after the June 15, 2026 Anthropic split

## What changed

Starting **June 15, 2026**, Anthropic splits paid Claude plans into two billing
pools:

| Pool                    | Sources                                                                                                                                  | Max 5x | Max 20x |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- | ------ | ------- |
| Interactive (existing)  | Claude Code interactive, claude.ai web/app, Claude Cowork                                                                                | unchanged subscription limits      | unchanged |
| **Programmatic (new)**  | Claude Agent SDK, `claude -p` (non-interactive), Claude Code GitHub Actions, **"third-party apps that authenticate with your Claude subscription through the Agent SDK"** | **$100/mo credit** | **$200/mo credit** |

Credits are per-user, reset monthly, do not roll over, are claimed once, and
drain first; when depleted, requests either stop or fall over to pay-as-you-go
("extra usage") API rates if the user opts in. API-key users see no change.

Sources: Anthropic help center article 15036540, Agent SDK overview docs,
VentureBeat / InfoWorld / Decoder coverage (May 2026 announcement).

## How Codey authenticates today

`src/auth.rs` and `src/llm/agent.rs`:

- OAuth client ID `9d1c250a-e61b-44d9-88ed-5944d1962f5e` (the Claude Code
  client ID). Set up by `codey --login`.
- Scopes `org:create_api_key user:profile user:inference`.
- The OAuth path (only entered when `self.oauth.is_some()`, agent.rs:583) sends
  requests directly against the public Messages API with:
  - `Authorization: Bearer <oauth.access_token>`
  - `anthropic-beta: oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14`
  - `user-agent: claude-code/2.1.37 (external, cli)`
- The non-OAuth path falls back to `ANTHROPIC_API_KEY` via `genai`'s default
  Anthropic adapter, or routes through OpenRouter when the model name is
  prefixed `openrouter::`. Neither sends the Claude Code beta/UA headers.

So the OAuth path *presents itself as Claude Code* on the wire and rides an
undocumented Claude-Code-only carve-out. The API-key and OpenRouter paths
don't, and post-June-15 they're completely unaffected by the split.

## The technical wall

The single most important fact for this research:

> **The public Messages API rejects OAuth tokens** from anything that doesn't
> look like Claude Code. The error is literally
> `"OAuth authentication is currently not supported."`

Issue `anthropics/claude-code#37205` requested official OAuth support on
`/v1/messages` and was **closed as not planned**. The only reason any OAuth
flow works at all is that Anthropic carves out an exception for traffic that
looks like Claude Code. That exception is gated by *multi-layer fingerprinting*
that the third-party-proxy projects have reverse-engineered:

1. **OAuth scope**: token must carry `user:inference` (from `claude setup-token`
   or the Claude Code OAuth flow).
2. **Beta header**: `anthropic-beta: …,claude-code-20250219,…`.
3. **User-agent**: `claude-code/<version> (external, cli)`.
4. **System-prompt fingerprint**: the API checks that the system prompt
   contains a specific ~84-character Claude Code billing identifier string.
   Without it, OAuth requests are routed to "Extra Usage" (pay-as-you-go API)
   billing rather than the subscription pool.
5. **Body "trigger phrases"**: an inbound streaming classifier looks at the
   request body for phrases that indicate a third-party tool; presence of those
   bumps you off the subscription path.

The OpenClaw episode in April 2026 was Anthropic tightening these. The
`zacdcook/openclaw-billing-proxy` project documents the four-layer detection
and lists explicit countermeasures — injecting the billing identifier into the
system prompt, replacing tool names to PascalCase, stripping ~30 trigger
phrases, and swapping in a Claude Code OAuth token. That's clearly out of
bounds: it's documented as "impersonating the official Claude Code CLI" and
exists specifically to evade Anthropic's enforcement.

Codey today implements layers 1–3 (and that's it). It does **not** inject the
84-char billing identifier into the system prompt, doesn't rename tools to
match Claude Code's, and doesn't sanitize trigger phrases. So even on the
OAuth path, Codey's traffic likely already falls into Extra Usage rather than
the subscription pool — and going further down the spoofing rabbit hole to
reach the subscription pool is explicitly the path that risks an account ban.

So the practical landscape post-June 15 looks like:

| Approach                                                | Subscription? | Hits new SDK credit? | Sanctioned? | First-class Rust? | Notes |
| ------------------------------------------------------- | ------------- | -------------------- | ----------- | ----------------- | ----- |
| Direct Messages API with OAuth, partial Claude-Code spoofing (status quo OAuth path) | Likely Extra Usage today | No  | Gray area | Yes | Carve-out is undocumented and conditional; only "works" for the interactive pool if you also spoof layers 4–5, which is the actively-enforced ban zone |
| Direct Messages API with OAuth + full fingerprint match (84-char id, tool renames, trigger-phrase scrubbing) | Yes — interactive | No  | **No** — explicit ToS violation, OpenClaw-style ban risk | Yes | Don't |
| Persistent `claude` binary sidecar (stdio JSON protocol, same shape the official SDK uses) | Yes  | **Yes**  | Yes — explicitly named in the announcement | No — subprocess | One spawn per session, not per turn; ~hundreds of ms startup, near-zero per-turn overhead; this is what `@anthropic-ai/claude-agent-sdk` is |
| Per-turn `claude -p` exec                              | Yes  | **Yes**  | Yes | No — subprocess | 3–5s per-turn overhead; simplest but worst UX |
| `ANTHROPIC_API_KEY` (pay-as-you-go)                    | No   | N/A      | Yes | Yes | Trivial, no policy risk, loses subscription-billing UX |
| OpenRouter (`openrouter::…`)                           | No   | N/A      | Yes (third-party) | Yes | Already in `src/llm/client.rs` |

### Why there is no first-class Rust path to the SDK credit pool

The SDK credit pool isn't gated by a documented header, scope, or endpoint.
It's gated by *being the Claude Code binary*, identified through the same
multi-layer fingerprint described above. The official Agent SDK (TypeScript
and Python) is itself a stdio wrapper around the bundled `claude` binary —
even Anthropic doesn't speak the wire protocol directly from their SDK code.

That means a "first-class" Rust implementation that legitimately draws from
the new SDK credit pool would require Anthropic to either:

- publish a stable wire-protocol contract for SDK-authenticated requests
  (currently they have not, and the closest documented endpoint, Managed
  Agents, is a different product with its own beta and pricing); or
- expose a new OAuth scope / API path for third-party SDK clients post-June-15
  (the announcement language hints this could come, but nothing has been
  published as of May 2026).

Neither exists today. Anthropic appears to have deliberately funneled
third-party access through the SDK subprocess rather than the wire protocol so
they retain enforcement leverage over fingerprint changes.

## What this means for Codey

> Exploration of the options. The chosen direction (see **Decision** below) is
> option 2, exclusively — option 1 is recorded here as the considered-and-
> rejected alternative.

The "make the SDK first-class in pure Rust, no subprocess" version doesn't
exist without crossing into spoofing. So the real choice is between two
sanctioned shapes, plus removing the spoofing risk on the OAuth path:

**1. Direct Messages API in Rust (status quo, cleaned up).** Drop OAuth, fall
back to `ANTHROPIC_API_KEY` only, strip the spoofed `user-agent` and
`claude-code-20250219` beta from `agent.rs:584–595`. Codey keeps its native
streaming agent loop, defines its own tools via the Messages API `tools`
parameter (which it already does via genai), and bills pay-as-you-go. Zero
policy risk. No subscription billing for end users.

**2. Persistent `claude` binary sidecar via the SDK control protocol.** Codey
spawns `claude` once per session and speaks to it over stdio NDJSON — the
exact same protocol the official `claude-agent-sdk-python` and
`@anthropic-ai/claude-agent-sdk` use. Per-turn cost is ~HTTP-equivalent; the
startup cost is paid once per session. This is the only sanctioned path onto
the new $100/$200 SDK credit pool.

The integration cost is lower than it first appears because the SDK control
protocol is explicitly designed to delegate work back to the host process.
The Python SDK's `_internal/query.py` demuxes three kinds of control requests
from the binary:

  1. **Tool permission requests** → `can_use_tool` callback
  2. **Hook callbacks** (`PreToolUse`, `PostToolUse`, etc.) via `HookMatcher`
  3. **MCP JSON-RPC messages** → in-process SDK MCP servers

That third channel is the key. The `@tool` decorator + `create_sdk_mcp_server`
in the Python SDK is **not** a separate process — it's a virtual MCP server
that lives in the SDK process. When the binary emits a `tool_use`, it sends
a JSON-RPC `tools/call` back over the *same stdio channel*, the SDK looks up
the matching `@tool`-decorated function, runs it in-process, and returns the
JSON-RPC response. The binary never sees the function — it just sees a tool
name and a result.

This means Codey can keep its entire existing tool registry — `read_file`,
`write_file`, `edit_file`, `shell`, `fetch_url`, `fetch_html`, `web_search`,
`open_file`, `spawn_agent`, `list_agents`, `get_agent`, `list_background_tasks`,
`get_background_task`, `record_correction` — in Rust, with the existing
permission UX, IDE/Neovim hooks, tool filters, fast mode, and sub-agent
registry, and surface all of them to the binary via the in-process MCP
bridge. The binary becomes purely: LLM client + agent loop + auth/billing +
wire-protocol. Codey owns everything else.

Concretely the design splits:

| Concern              | Where it lives                                                  |
| -------------------- | --------------------------------------------------------------- |
| OAuth / auth refresh | binary (`claude --login`, `~/.claude/.credentials.json`)        |
| Billing routing      | binary (the whole point)                                        |
| Wire protocol to API | binary                                                          |
| Agent loop / context | binary                                                          |
| System prompt        | binary base + `--append-system-prompt` for Codey additions      |
| Sub-agents           | `--agents` flag, definitions translated from Codey's registry   |
| Tool registry        | **Codey**, exposed via in-process MCP server over stdio JSON-RPC|
| Tool permission UX   | **Codey**, via `--permission-prompt-tool stdio` callbacks       |
| IDE/Neovim hooks     | **Codey**, in the tool implementations                          |
| Fast mode            | `--settings '{"fastMode": true}'` (confirmed); binary handles cooldown/fallback; bills to usage credits, not the SDK pool |
| Transcript / session | binary writes `~/.claude/sessions/*.jsonl`; Codey can mirror via `SessionStore` analog or its own |

### Existing Rust crates

There are several Rust crates in this space. The decision that matters: the
full-SDK ports are unmaintained, and the one actively maintained crate is the
lower-level protocol+client layer.

| Crate | Ver | Last pub | Downloads | License | Scope | Maintained? |
| ----- | --- | -------- | --------- | ------- | ----- | ----------- |
| claude-codes (meawoppl) | 2.1.141 | 2026-05-18 | ~4.2k | Apache-2.0 | Subprocess + protocol + control-msg types | **Yes — every 1–2 wks, CLI-version-pinned** |
| claude-agent-sdk-rs (tyrchen) | 0.6.4 | 2026-02-09 | ~15.5k | MIT | Full SDK (`tool!` macro, callbacks) | No — quiet since Feb |
| claude-code-agent-sdk (soddygo) | 0.1.39 | 2026-02-13 | ~2.1k | MIT | Full SDK (parallel effort) | No — quiet since Feb |
| claude-sdk-rs (bredmond1019) | 1.0.2 | 2025-11-07 | ~1.9k | MIT | Full SDK (older) | No — quiet since Nov |
| claude-code-rs, claude-code-client-sdk | early | — | <100 | — | unproven | n/a |

**No actively-maintained full Rust Agent SDK exists.** Both full-SDK ports
(tyrchen, soddygo) went quiet in February. Building the backend on either means
adopting an unmaintained dependency in a critical path — not acceptable for a
core runtime.

**`claude-codes` is the right foundation,** and it's more capable than its
repo README ("typed interfaces") implies. The published 2.1.141 crate exposes:

- `client_async::AsyncClient` / `client_sync::SyncClient` that spawn and manage
  the `claude` subprocess, with `ResponseStream` / `AsyncStreamProcessor` for
  typed streaming.
- An `io` module defining the full control protocol: `ControlRequest`,
  `ControlResponse`, `ToolPermissionRequest`, `McpMessageRequest`,
  `HookCallbackRequest`.
- `protocol` / `messages` / `types` / `tool_inputs` — the serde layer.
- `version` — CLI version checking, and the **crate version literally tracks
  the CLI version it's tested against** (2.1.141 ↔ CLI 2.1.141), with
  divergence warnings. This is the direct fix for the protocol-drift caveat.

What it does *not* provide is the higher-level sugar — no `tool!` macro, no
permission-callback registration. It hands you the typed control message and
you respond. **For Codey that's the ideal split, not a gap:**

| Layer | Owner | Why |
| ----- | ----- | --- |
| Subprocess spawn, JSON-Lines framing, protocol/message types, CLI version pinning | `claude-codes` | Mechanical, drift-prone, actively maintained; low lock-in (Apache-2.0, easy to vendor) |
| Control-request loop: answer `ToolPermissionRequest` via Codey's permission UI; dispatch `McpMessageRequest` (`tools/list`, `tools/call`) to Codey's `ToolRegistry`; handle `HookCallbackRequest` | **Codey** | The opinionated glue Codey wants to own — integrates the existing registry, permission filters, IDE/Neovim hooks |
| Adapter from the output `messages` stream into `AgentStep` | **Codey** | Same `AgentStep` shape `app.rs` already consumes |

The only thing tyrchen's full SDK adds over this is the `tool!` macro and
callback registration — exactly the layer Codey would replace with direct
`ToolRegistry` / permission-UI integration anyway. So depending on the
unmaintained crate buys sugar that gets thrown away; depending on
`claude-codes` buys the maintained mechanical layer and leaves Codey to write
the stable, opinionated glue. tyrchen's crate stays useful as an MIT
*reference* for the control-loop shape — cribbed, not depended on.

Residual risk: `claude-codes` is also a single-maintainer crate. But the
dependency is on framing + types (mechanical, CLI-pinned, trivially
vendorable), not on behavior — far lower lock-in than depending on a full
SDK's agent logic.

### Shape of the Codey-owned glue

The control loop Codey writes on top of `claude-codes` maps to the Python
SDK's `_internal/query.py` (the reference for the demux):

- `_internal/transport/subprocess_cli.py` → a transport crate:
- Subprocess spawn, NDJSON framing, version check, the ~15 output event
  variants and the control-protocol message types → **provided by
  `claude-codes`** (`AsyncClient`, `io::*`, `protocol`, `messages`, `version`).
  Not Codey's code to write or maintain.
- `_internal/query.py`'s demux → the loop Codey writes: read the `AsyncClient`
  stream, and on each `ControlRequest` branch on its variant —
  `ToolPermissionRequest` → Codey's permission UI → `ControlResponse`;
  `McpMessageRequest` (`tools/list`, `tools/call`) → Codey's `ToolRegistry`
  → `ControlResponse`; `HookCallbackRequest` → transcript/fast-mode handling.
  Output `messages` get mapped into the event type `agent.rs` already consumes.
- The MCP bridge is just the `McpMessageRequest` arm of that loop: translate
  `tools/list` and `tools/call` into `ToolRegistry` calls. No listening socket,
  no separate process — dispatch over the existing control channel.

### Spawn line

Roughly:

```
claude --bare -p \
  --output-format stream-json --input-format stream-json \
  --verbose --include-partial-messages \
  --permission-prompt-tool stdio \
  --allowedTools "<comma-sep list of Codey's tool names>" \
  --append-system-prompt-file <codey's SYSTEM.md> \
  --mcp-config <codey-as-mcp-server config>
```

`--bare` skips OAuth/keychain (auth still comes from the binary's logged-in
state — `--bare` just suppresses re-prompting), skips auto-discovery of
hooks/skills/plugins/CLAUDE.md so the spawn is deterministic, and per the
docs is "the recommended mode for scripted and SDK calls."

### Caveats worth tracking

- **`--input-format stream-json` is intentionally undocumented**
  (`anthropics/claude-code#24594` is open). The output side is documented,
  the input side is reverse-engineered. This is largely handled for us:
  `claude-codes` tracks the CLI version it's tested against and warns on
  divergence, so the drift surface is owned by a maintained crate rather than
  Codey. Still worth pinning a known-good `claude` version range in Codey's
  docs and surfacing the crate's divergence warning to the user.
- **Min CLI version is enforced** by `claude-codes` (`version` module). No
  separate handling needed in Codey beyond surfacing the error.
- **No binary bundling.** `claude-codes` uses the system-installed `claude`,
  so Codey requires users to install it themselves (`brew install
  anthropic/claude/claude` or the install script). Add a setup-doc note and a
  clear "claude not found / wrong version" error path.

**3. Status quo (partial Claude-Code spoofing on the OAuth path).** Today
this almost certainly already falls into Extra Usage rather than the
subscription pool because Codey doesn't inject the 84-char system-prompt
identifier. So you're getting the spoofing risk without the billing benefit.
Strip the `claude-code-20250219` beta and `claude-code/2.1.37 (external, cli)`
user-agent from the OAuth path — they buy nothing and are the visible
enforcement signal.

## Decision: standardize on a single Anthropic-SDK backend

The exploration above lays out the options; the chosen direction is to collapse
them. Codey migrates its **entire** backend path onto the Anthropic Agent SDK
driven through the `claude` binary (via `claude-codes`), and removes every other
path.

1. **One backend.** The `claude` binary is the only LLM path. Delete the genai
   dependency (`tcdent/rust-genai`), `src/llm/client.rs`, the `openrouter::`
   routing, and the hand-rolled direct-HTTP idea. There is no `DirectAgent` and
   no second runtime — just the SDK backend.
2. **Billing is the binary's problem, not Codey's.** The binary accepts either
   the subscription OAuth session (→ the new SDK credit pool) *or*
   `ANTHROPIC_API_KEY` (→ pay-as-you-go), via its own auth precedence. So both
   billing methods are covered by the single backend, and Codey no longer owns
   authentication: `src/auth.rs` (the OAuth/PKCE flow, token storage, refresh)
   is removed, and `codey --login` either goes away or delegates to
   `claude --login`. No spoofed headers, no token management.
3. **The binary is a hard dependency — everywhere, including library mode.**
   Codey-as-a-library consumers also run on the SDK backend and also require
   `claude` installed. This is an accepted requirement, not a problem to design
   around. Codey must keep working as a library (other projects depend on it),
   so the library API stays — it just runs on the SDK backend like the CLI.
4. **No generic multi-provider abstraction.** A universal "any LLM" adapter
   isn't worth it — tool use, thinking, caching, and the other Anthropic
   specifics carry too much subtlety, and reapplying all of that to something
   like litellm is verbose, tedious, and test-heavy. (The genai/OpenRouter path
   was an experiment that was hacked in; it's dropped without regret.) The
   *internal contract* between Codey and the backend should still be a clean,
   somewhat-abstract seam over Codey-owned types — generic enough not to leak
   the SDK's internals upward, but not engineered to span providers.

What this removes from the tree: `genai` + `src/llm/client.rs`, `src/auth.rs`,
OpenRouter routing, most of `agent.rs`'s loop, `src/compaction.rs`, the
`cache_control` logic, and thinking-block bookkeeping — all now owned by the
binary. Codey gets meaningfully smaller.

## Integration into Codey (migrating to the SDK backend)

The seam is well-placed, but the work is real: **the `claude` binary owns the
agent loop that `src/llm/agent.rs` owns today.**
what "replace the Anthropic wrapper" actually means.

### Where the layers sit today

- **genai (being removed)** is a *stateless per-turn streaming client*:
  `exec_chat_stream(model, request, options) -> stream of events`. It holds no
  conversation state, so `src/llm/agent.rs` had to hold all of it.
- **`src/llm/agent.rs` owns the agent loop today.** The `Agent` struct holds
  `messages: Vec<ChatMessage>` (full history), the `StreamState` machine
  (`NeedsChatRequest` → `Streaming` → `AwaitingToolDecision`), accumulates
  text/thinking/tool-calls during a stream, and in `submit_tool_result()`
  rebuilds the assistant message + tool responses and loops back to
  `NeedsChatRequest` for the continuation. It also owns `restore_from_transcript`,
  `reset_with_summary` (compaction), prompt caching (`CacheControl::Ephemeral`),
  thinking-block preservation, and OAuth.
- The **`claude` binary owns that entire loop**: session/history (in
  `~/.claude`), the turn loop, tool orchestration, context management,
  auto-compaction, caching, thinking handling, and auth.

So the migration doesn't just delete `src/llm/client.rs` — it deletes
`client.rs` **plus most of `agent.rs`'s loop, plus message-history management,
plus compaction**, because the binary now does all of that. A loop transplant,
not a wrapper swap.

### The seam is well-placed, so the rest of Codey is mostly untouched

genai types appear in only two files: `src/llm/agent.rs` and
`src/llm/client.rs`. Everything above the `Agent` interface is already
backend-agnostic:

- `app.rs` drives agents purely through `send_request()` / `next() ->
  AgentStep` / `submit_tool_result()` and never names a genai type.
- `AgentStep` and `ToolCall` are Codey-native (`crate::tools`), not genai.
- Tools are already Codey-native (`ToolRegistry`, `ToolExecutor`) and the tool
  names are already MCP-style (`mcp_read_file`, `mcp_shell`, …).
- The `Transcript` is Codey's own model.

### The contract: one backend behind a clean Codey-owned seam

There is a single implementation — the SDK backend — but it's worth defining
the `Agent` interface as a deliberate **contract over Codey-owned types**
rather than letting the backend's types leak upward. That's the "somewhat more
generic" seam without the multi-provider tax:

- **Input side:** `send_request(prompt, mode)`, `submit_tool_result(call_id,
  content)`, `cancel()` — already Codey-owned.
- **Output side:** the `AgentStep` stream (`TextDelta`, `ThinkingDelta`,
  `ToolRequest`, `Finished { usage }`, `Error`, `Retrying`) — already
  Codey-owned.
- **Tool advertisement:** from the `ToolRegistry` (name/description/schema) —
  already Codey-owned; rendered to MCP descriptors for the binary instead of
  genai `Tool`s.

Nothing in that contract names Anthropic or the binary. The SDK backend is the
only thing behind it, but the rest of Codey (TUI, tools, transcript, library
API) depends only on the contract. If a different backend ever became
necessary, it'd implement the same contract — but the contract is **not**
designed by surveying providers for common surface (the explicitly-rejected
approach); it's designed around what Codey already needs and already passes
across this boundary.

### The `AgentStep` round-trip maps cleanly onto the SDK control protocol

This is the part that makes it tractable. The SDK backend translates:

| Binary event (via `claude-codes`)             | Codey `AgentStep` / action |
| --------------------------------------------- | -------------------------- |
| assistant text delta                          | `AgentStep::TextDelta`     |
| thinking delta                                | `AgentStep::ThinkingDelta` |
| `McpMessageRequest` `tools/call` / permission request | `AgentStep::ToolRequest(..)` |
| (app approves, `ToolExecutor` runs Codey's tool, `submit_tool_result`) | write the `tools/call` MCP response / permission allow back over the control channel |
| `result`                                      | `AgentStep::Finished { usage }` (usage from the result event) |
| error event                                   | `AgentStep::Error` / `Retrying` |

So the existing `ToolRequest` → `tool_executor` → `submit_tool_result` path —
including approvals, IDE diff previews, and background tasks — works as-is. The
difference is internal: the SDK backend's `submit_tool_result` writes an MCP
response over stdio instead of rebuilding `messages` and re-calling the API.

### What moves into the binary and is deleted from Codey

- `messages: Vec<ChatMessage>` history → binary session; use `--resume
  <session_id>`. Codey still records the streamed events into its own
  `Transcript` for display/persistence/library access, but it no longer
  replays history into requests.
- `StreamState` continuation logic in `submit_tool_result` → binary's loop.
- `restore_from_transcript` → binary session import / `--resume`.
- Compaction: `RequestMode::Compaction`, `reset_with_summary`,
  `src/compaction.rs`, and the `compaction_threshold` check in the `Finished`
  handler → binary auto-compaction (`PreCompact` hook). Codey's compaction code
  is removed.
- `CacheControl::Ephemeral`, thinking-block preservation → binary-owned.
- Auth: `src/auth.rs` (OAuth/PKCE, token storage, `refresh_oauth_if_needed`),
  the `oauth` field on `Agent`, and the spoofed beta/user-agent headers → all
  removed. The binary owns auth (subscription OAuth or `ANTHROPIC_API_KEY`).

### What's at risk or needs remapping

- **Fast mode — reachable, confirmed.** Codey added fast mode by reverse-
  engineering the `research-preview-2026-02-01` beta header; the binary has it
  natively. `/fast` is interactive-only, **but the docs give a second path:
  `"fastMode": true` in the settings file**, and `claude -p` accepts
  `--settings <file-or-json>`. So Codey enables it by spawning with
  `--settings '{"fastMode": true}'` (works even under `--bare`, which otherwise
  skips settings discovery). Caveats:
  - **Billing:** fast mode draws from **usage credits (pay-as-you-go,
    $30/$150 MTok), not the SDK credit pool** — "fast mode tokens do not count
    against your plan's included usage and are charged at the fast mode rate
    from the first token." The account must have *usage credits turned on*
    (admin-gated on Team/Enterprise). So fast mode is a separate, explicit cost
    even on the subscription backend.
  - **Model:** enabling fast mode auto-switches to Opus; the default is now
    Opus **4.7** (v2.1.142+). Codey currently gates fast mode to opus-4-6 —
    set `CLAUDE_CODE_OPUS_4_6_FAST_MODE_OVERRIDE=1` to preserve that.
  - **Simplification:** the binary natively handles fast-mode rate-limit
    fallback, cooldown, and auto-re-enable — exactly what `agent.rs` hand-rolls
    (`FAST_MODE_COOLDOWN`, `is_rate_limit_error`, the cooldown branch). That
    logic is deleted, not ported.
  - Requires Claude Code v2.1.36+; not available on Bedrock/Vertex/Azure (fine
    for Codey's direct-Anthropic case).
- **`thinking_budget: u32`** → the binary uses effort *levels* (`low`…`xhigh`),
  not a raw token budget, so Codey's budget maps to an effort bucket.
- **`max_tokens`, `model`** → query options / `--settings`. The binary only
  routes Anthropic models; with OpenRouter intentionally dropped, that's fine.
- **Usage accounting** — `extract_turn_usage` is reimplemented against the
  binary's `result` event (`total_cost_usd`, token counts); the cache-token
  breakdown may differ in shape from the genai usage it replaces.
- **Sub-agents** — Codey's `spawn_agent` + `AgentRegistry` spawn independent
  `Agent`s today. Simplest mapping: each Codey sub-agent is its own `claude`
  subprocess (matches the current independent-agent model; costs N processes).
  Alternative: drive the binary's native `--agents` and bridge via
  `parent_tool_use_id` — fewer processes, more bridging work.

### Config

`AgentRuntimeConfig` (today: `model`, `max_tokens`, `thinking_budget`,
`compaction_thinking_budget`, `fast_mode`) loses the genai-era knobs that the
binary now owns (caching is implicit; `compaction_thinking_budget` is the
binary's concern). No `runtime` selector is needed — there's one backend.
`model` and effort still pass through; `fast_mode` depends on the investigation
above. Per-agent config (foreground vs background) stays, e.g. different models
or effort levels per agent.

### Net

This is a single migration, not an addition: `agent.rs` and `client.rs` are
rewritten against `claude-codes`, and a chunk of Codey is deleted along the way
(`auth.rs`, `compaction.rs`, genai dep, OpenRouter routing, the history/caching
machinery). Budget the work as:

1. A `claude-codes`-backed `Agent` implementation: spawn/manage the binary,
   stream events, and a control-request loop that routes `ToolPermissionRequest`
   → Codey's permission UI and `McpMessageRequest` → `ToolRegistry`.
2. Defining the `Agent` trait contract over Codey-owned types so `app.rs`,
   `ToolExecutor`, `ToolRegistry`, `Transcript`, and the library API depend only
   on it — not on `claude-codes` internals.
3. Deleting the now-dead paths (auth, compaction, caching, history replay,
   OpenRouter) and confirming the fast-mode story.

`app.rs`, `ToolExecutor`, `ToolRegistry`, and `Transcript` are otherwise
unaffected — they already sit above the `Agent` seam.

## Open questions worth confirming before implementing

- Whether Anthropic publishes a documented OAuth scope / endpoint / beta flag
  for third-party SDK-billing access on or around June 15. The announcement
  language ("third-party apps that authenticate with your Claude subscription
  through the Agent SDK") implies a sanctioned mechanism exists; right now
  the only mechanism is "be a subprocess of the binary." A direct support
  ticket asking whether a Rust client can target the SDK credit pool natively
  would resolve this — the public docs are silent.
- Whether the Agent SDK subprocess can be driven without a separate `claude`
  binary install. The TypeScript SDK ships a bundled binary as an optional
  npm dep, so `npm i @anthropic-ai/claude-agent-sdk` may be sufficient — that
  would let Codey ship a "subscription backend" install step that's one npm
  command rather than a full Claude Code installation.
- Whether the SDK's stdio protocol exposes enough hooks (`PreToolUse`,
  `agents`, `permissionMode`, `mcpServers`, `appendSystemPrompt`) to keep
  Codey's existing UX intact — tool permissions, IDE diff previews, sub-agent
  registry, fast mode. Resolved for most of these: tools and permissions are
  fine via the in-process MCP bridge + `--permission-prompt-tool stdio`, and
  sub-agents are fine via `--agents`. Open: how fast mode (Codey's
  `research-preview-2026-02-01` beta) interacts with the binary's wire-level
  beta header set — likely needs `--settings` JSON or isn't reachable at all
  through the SDK protocol.
- How session persistence reconciles. The binary writes its own session JSONL
  to `~/.claude/sessions/`; Codey writes `.codey/transcripts/`. Either Codey's
  transcript becomes a render of the binary's session (via the documented
  `SessionStore` protocol in the Python SDK), or Codey keeps its own format
  and ignores the binary's. Worth deciding before implementing `--resume`
  / `--continue`.

## References

- https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan
- https://code.claude.com/docs/en/agent-sdk/overview
- https://code.claude.com/docs/en/fast-mode (fast mode via `"fastMode": true`; usage-credit billing; Opus 4.7 default)
- https://code.claude.com/docs/en/headless (`--settings <file-or-json>`, `--bare`, stream-json)
- https://code.claude.com/docs/en/env-vars (`CLAUDE_CODE_OPUS_4_6_FAST_MODE_OVERRIDE`, `CLAUDE_CODE_DISABLE_FAST_MODE`)
- https://github.com/anthropics/claude-code/issues/37205
- https://github.com/anthropics/claude-code/issues/24594 (open issue requesting docs for `--input-format stream-json`)
- https://github.com/anthropics/claude-agent-sdk-python/tree/main/src/claude_agent_sdk (reference architecture for a Rust port)
- https://github.com/meawoppl/rust-code-agent-sdks (`claude-codes`; actively maintained, CLI-version-pinned subprocess+protocol layer — recommended foundation)
- https://github.com/tyrchen/claude-agent-sdk-rs (full Rust port; unmaintained since 2026-02 — reference only, do not depend on)
- https://github.com/zacdcook/openclaw-billing-proxy (documents the four-layer detection; do not emulate)
- https://github.com/majdyz/openclaw-claude-proxy (explicitly shells out to `claude` to avoid spoofing)
- https://venturebeat.com/technology/anthropic-reinstates-openclaw-and-third-party-agent-usage-on-claude-subscriptions-with-a-catch
- https://www.infoworld.com/article/4171274/anthropic-puts-claude-agents-on-a-meter-across-its-subscriptions.html
- https://the-decoder.com/claude-subscriptions-get-separate-budgets-for-programmatic-use-billed-at-full-api-prices/
- https://docs.openclaw.ai/providers/anthropic
