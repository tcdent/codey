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
| Fast mode            | needs investigation — likely `--settings` JSON                  |
| Transcript / session | binary writes `~/.claude/sessions/*.jsonl`; Codey can mirror via `SessionStore` analog or its own |

### Shape of a Rust port

The Python SDK source maps cleanly onto Rust crates:

- `_internal/transport/subprocess_cli.py` → a transport crate:
  `tokio::process::Child`, `LinesCodec` for NDJSON framing, binary discovery
  (bundled path → `$PATH` → common install dirs), version check against a
  pinned min `claude --version`, graceful shutdown with timeout + force-kill
  fallback, concurrent-write mutex on stdin.
- `_internal/query.py` → a `Query`-equivalent: an async task that reads stdout
  events, demuxes control requests into three channels (permission callback,
  hook callback, MCP JSON-RPC handler), and exposes an async `Stream` of
  typed events that `agent.rs` consumes the same way it consumes genai
  events today.
- `_internal/types.py` → serde structs/enums for ~15 event variants
  (`system/init`, `system/api_retry`, `system/plugin_install`, `assistant`,
  `user` with tool_result, `stream_event`, `result`, `control_request`,
  `control_response`).
- `create_sdk_mcp_server` + `@tool` → a tiny JSON-RPC handler that translates
  `tools/list` and `tools/call` into Codey's existing `ToolRegistry` calls.
  No listening socket, no separate process — just dispatch over the stdio
  control channel.

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
  the input side is reverse-engineered from the SDKs. Pin a known-good
  `claude` version range and detect drift on startup.
- **Min CLI version is enforced.** The Python SDK pins `>= 2.0.0` and refuses
  to start otherwise. A Rust port should do the same.
- **The bundled binary is a per-platform `optionalDependencies` npm package**
  in the TypeScript SDK case. For a Rust port, simplest is to require users to
  install `claude` themselves (`brew install anthropic/claude/claude` or the
  install script), with a setup-doc note that links to it.

**3. Status quo (partial Claude-Code spoofing on the OAuth path).** Today
this almost certainly already falls into Extra Usage rather than the
subscription pool because Codey doesn't inject the 84-char system-prompt
identifier. So you're getting the spoofing risk without the billing benefit.
Strip the `claude-code-20250219` beta and `claude-code/2.1.37 (external, cli)`
user-agent from the OAuth path — they buy nothing and are the visible
enforcement signal.

## Recommendation

Given you've already been steering away from spoofing:

1. **Strip the spoofed `user-agent` and `claude-code-20250219` beta from the
   OAuth path in `src/llm/agent.rs:584–595`.** They give nothing back
   (requests likely already land in Extra Usage, not the subscription pool)
   and they're the visible signal that invites enforcement.
2. **Default to `ANTHROPIC_API_KEY`** via the existing genai Anthropic
   adapter. Document that subscription billing requires picking the
   `claude-sdk` runtime once it's built.
3. **Build the `claude` subprocess backend as a real second runtime,** behind
   config like `runtime = "claude-sdk"` per agent — the same shape that
   already distinguishes `direct` Anthropic from `openrouter::`. Codey's
   existing `ToolRegistry` plugs into the binary via the in-process MCP
   bridge described above, so almost nothing about the rest of the codebase
   changes. The Rust port follows the Python SDK structure file-for-file in a
   small transport crate.
4. **Track Anthropic's June-15 announcement** for any new documented OAuth
   scope, endpoint, or beta flag that would expose the SDK credit pool to
   native HTTP clients. If/when that lands, a third runtime can be added; for
   now, the subprocess path is the only sanctioned route to the credit pool.

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
- https://github.com/anthropics/claude-code/issues/37205
- https://github.com/anthropics/claude-code/issues/24594 (open issue requesting docs for `--input-format stream-json`)
- https://github.com/anthropics/claude-agent-sdk-python/tree/main/src/claude_agent_sdk (reference architecture for a Rust port)
- https://github.com/zacdcook/openclaw-billing-proxy (documents the four-layer detection; do not emulate)
- https://github.com/majdyz/openclaw-claude-proxy (explicitly shells out to `claude` to avoid spoofing)
- https://venturebeat.com/technology/anthropic-reinstates-openclaw-and-third-party-agent-usage-on-claude-subscriptions-with-a-catch
- https://www.infoworld.com/article/4171274/anthropic-puts-claude-agents-on-a-meter-across-its-subscriptions.html
- https://the-decoder.com/claude-subscriptions-get-separate-budgets-for-programmatic-use-billed-at-full-api-prices/
- https://docs.openclaw.ai/providers/anthropic
