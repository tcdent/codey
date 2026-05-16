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

The "make the SDK first-class, no subprocess" version of this doesn't exist
without crossing into spoofing — see the previous section. So the real choice
is between three sanctioned shapes:

**1. API-key default, drop OAuth.** Simplest. Users bring an `sk-ant-...`
API key, requests go pay-as-you-go (or against the new SDK credit pool *if*
Anthropic later opens a documented OAuth path for it — TBD). Zero policy
risk, no subprocess, native Rust streaming, but no subscription billing for
end users.

**2. Persistent `claude` binary sidecar as a new backend.** Codey spawns
`claude` (or `@anthropic-ai/claude-agent-sdk`) once per session and speaks
to it over the stdio JSON protocol — same shape the official SDKs use. Per
turn this is roughly as fast as direct HTTP; the startup cost is paid once.
Subprocess in the literal sense, but not "shell out per turn". This is the
only sanctioned path onto the new SDK credit pool. Cost: significant
integration work, because Codey's tool registry, IDE/Neovim hooks, permission
system, fast-mode handling, and sub-agent registry overlap with what the SDK
expects to own. Each one needs to be either delegated to the SDK
(`PreToolUse` hook, `agents` option, `permissionMode`) or kept in Codey with
the SDK's matching feature disabled.

**3. Status quo (partial Claude-Code spoofing on the OAuth path).** Today
this almost certainly already falls into Extra Usage rather than the
subscription pool because Codey doesn't inject the 84-char system-prompt
identifier. So you're getting the spoofing risk without the billing benefit.
If preserving the OAuth login UX matters, keeping it is fine; if it doesn't,
the cleanest move is to remove the `claude-code-20250219` beta and
`claude-code/2.1.37 (external, cli)` user-agent and let the OAuth path
explicitly fall back to whatever Anthropic does with an unbranded OAuth
request (currently: reject). That's a controlled deprecation rather than
silently leaning on an enforcement carve-out.

## Recommendation

Given you've already been steering away from spoofing:

1. **Strip the spoofed `user-agent` and `claude-code-20250219` beta from the
   OAuth path now.** They give nothing back (the request likely isn't even
   landing on the subscription pool) and they're the visible signal that
   most invites enforcement action.
2. **Add an `ANTHROPIC_API_KEY`-first default**, with clear docs that
   subscription users who want subscription billing should pick option 3.
3. **Build option 2 (persistent `claude` subprocess backend) as a real
   second runtime,** behind config like `runtime = "claude-sdk"` per agent —
   the same shape that already distinguishes `direct` Anthropic from
   `openrouter::`. This is genuinely the only sanctioned route to the new
   $100/$200 SDK credit pool, and being a subprocess in the per-session sense
   is acceptable.
4. **Track Anthropic's June-15 announcement closely** for any new documented
   OAuth scope, endpoint, or beta flag that would expose the SDK credit pool
   to native HTTP clients. If/when that lands, the persistent-subprocess
   backend can be paralleled by a native Rust backend; today, it can't.

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
  registry, fast mode. If any of those would have to regress, that's the
  real cost of the sidecar approach.

## References

- https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan
- https://code.claude.com/docs/en/agent-sdk/overview
- https://github.com/anthropics/claude-code/issues/37205
- https://github.com/zacdcook/openclaw-billing-proxy (documents the four-layer detection; do not emulate)
- https://github.com/majdyz/openclaw-claude-proxy (explicitly shells out to `claude` to avoid spoofing)
- https://venturebeat.com/technology/anthropic-reinstates-openclaw-and-third-party-agent-usage-on-claude-subscriptions-with-a-catch
- https://www.infoworld.com/article/4171274/anthropic-puts-claude-agents-on-a-meter-across-its-subscriptions.html
- https://the-decoder.com/claude-subscriptions-get-separate-budgets-for-programmatic-use-billed-at-full-api-prices/
- https://docs.openclaw.ai/providers/anthropic
