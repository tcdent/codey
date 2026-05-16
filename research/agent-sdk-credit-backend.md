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
  client ID).
- Scopes `org:create_api_key user:profile user:inference`.
- Outgoing requests use `Authorization: Bearer <oauth.access_token>` directly
  against the public Messages API, with:
  - `anthropic-beta: oauth-2025-04-20,claude-code-20250219,interleaved-thinking-2025-05-14`
  - `user-agent: claude-code/2.1.37 (external, cli)`
- Non-OAuth path falls back to the `ANTHROPIC_API_KEY` env var via `genai`'s
  default Anthropic adapter, or routes through OpenRouter when the model name
  is prefixed `openrouter::`.

So today Codey *presents itself as Claude Code* on the wire. Anthropic's
billing pipeline almost certainly classifies that traffic as Claude Code, which
means it currently lands in the **interactive** pool described above.

## The technical wall

The single most important fact for this research:

> **The public Messages API rejects OAuth tokens.** The error is literally
> `"OAuth authentication is currently not supported."`

Issue `anthropics/claude-code#37205` requested official OAuth support on
`/v1/messages` and was **closed as not planned**. The only reason Codey's
OAuth flow works at all is that Anthropic carves out an exception for traffic
that looks like Claude Code (specific beta header + UA + scope combination).
That exception is undocumented, conditional on Anthropic's discretion, and was
publicly tightened earlier in 2026 (the "OpenClaw ban" episode).

So the practical landscape post-June 15 looks like:

| Approach                                                | Subscription? | Hits new SDK credit? | Sanctioned?          | Notes |
| ------------------------------------------------------- | ------------- | -------------------- | -------------------- | ----- |
| Direct Messages API with OAuth, presenting as Claude Code (status quo) | Yes — interactive pool | No                   | Gray area; relies on undocumented carve-out | Can be revoked unilaterally; user-agent spoofing is exactly the pattern Anthropic has cracked down on |
| Shell out to `claude -p` (Agent SDK CLI subprocess)     | Yes           | **Yes**              | Yes — explicitly named in the announcement | Requires `claude` binary installed on the user's box; adds 3-5s overhead per turn; loses fine-grained streaming/tool-loop control |
| Wrap the Node/Python Agent SDK in a sidecar process     | Yes           | **Yes**              | Yes — "third-party apps that authenticate through the Agent SDK" | Same install footprint as above; cleaner JSON protocol via SDK stdio, but still a subprocess hop |
| `ANTHROPIC_API_KEY` (pay-as-you-go)                     | No            | N/A                  | Yes                  | Trivial, no policy risk, loses subscription-billing UX |
| OpenRouter (`openrouter::…`)                            | No            | N/A                  | Yes (third-party)    | Already implemented in `src/llm/client.rs`; passes through OpenRouter billing |

## What this means for Codey

The June 15 change does not *force* Codey to do anything — the question is
which of these tradeoffs to pick.

**1. Doing nothing.** Codey continues to ride the Claude Code carve-out, and
post-June 15 that traffic continues hitting the interactive subscription pool
(shared with the user's chat / Claude Code sessions). Pros: zero work,
preserves the streaming Rust agent loop. Cons: not on the new $100–$200 SDK
credit; exposed to enforcement at any time; spoofing `user-agent` as
`claude-code/2.1.37` to access the Claude Code-only API path is the kind of
thing the AVP team has shown willingness to break.

**2. Add a `claude-cli` backend.** Following the OpenClaw / Conductor /
Zed pattern: when the user opts in, Codey shells out to a long-running
`claude` (or `@anthropic-ai/claude-agent-sdk`) subprocess, talks to it over
stdio JSON, and forwards its events into the existing UI. Pros: officially
sanctioned, draws from the new dedicated SDK credit pool (an extra $200/mo
of headroom for Max 20x users on top of the interactive pool), survives any
future tightening of the OAuth carve-out. Cons: requires the `claude` binary
or Node SDK on the user's system, adds subprocess latency, and the tool loop
is partly owned by the SDK — Codey's own tool registry, IDE/Neovim hooks,
permission system, fast-mode handling, and sub-agent registry would have to
either delegate to the SDK's equivalents or be layered on top.

**3. Add only the API-key path (deprecate OAuth).** Cleanest legally,
worst UX — users on Pro/Max who are already paying Anthropic suddenly have
to also fund a developer-console balance to use Codey.

## Recommendation

Treat backends as a choice the user makes, not a single rewrite:

1. **Keep the current direct-OAuth backend** as the default for now. Document
   that it's running against the interactive subscription pool and is subject
   to Anthropic policy. No code change required for June 15 itself.
2. **Add a `claude-cli` backend** as an opt-in second backend (config:
   `runtime = "claude-cli"` per agent, in addition to today's implicit
   `direct` runtime). This is the only path that's explicitly named as
   eligible for the new credit pool and the only path that's robust to
   Anthropic revoking the Claude Code carve-out for non-Claude-Code clients.
   The existing two-backend pattern (`direct` Anthropic vs `openrouter::`
   prefix) already establishes the precedent for routing per request.
3. **Leave API-key fallback alone** — already works via genai's default
   adapter when `ANTHROPIC_API_KEY` is set and OAuth isn't.

The thing to weigh before committing to (2) is how much of Codey's value
(streaming UX, native tool registry, IDE diff previews, sub-agent registry,
fast-mode) survives running on top of a `claude` subprocess vs being
re-implemented as SDK hooks/agents. That's the architectural call, and it's
the actual question hiding behind "which backend".

## Open questions worth confirming before implementing

- Whether Anthropic intends the existing `user-agent: claude-code/(external, cli)`
  + `oauth-2025-04-20` beta path to keep working after June 15 for non-Claude-Code
  clients, or whether the carve-out is being narrowed as part of the split.
  Worth a direct support ticket — the public docs don't say.
- Whether the Agent SDK subprocess can be driven without the full `claude`
  binary install (the TypeScript SDK ships a bundled binary as an optional
  dep, so `npm i @anthropic-ai/claude-agent-sdk` may be sufficient).
- Whether the SDK's stdio protocol exposes enough to keep Codey's existing
  tool-permission UX (the `PreToolUse` hook seems sufficient, but worth
  verifying against current tool flow).

## References

- https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan
- https://code.claude.com/docs/en/agent-sdk/overview
- https://github.com/anthropics/claude-code/issues/37205
- https://venturebeat.com/technology/anthropic-reinstates-openclaw-and-third-party-agent-usage-on-claude-subscriptions-with-a-catch
- https://www.infoworld.com/article/4171274/anthropic-puts-claude-agents-on-a-meter-across-its-subscriptions.html
- https://the-decoder.com/claude-subscriptions-get-separate-budgets-for-programmatic-use-billed-at-full-api-prices/
- https://docs.openclaw.ai/providers/anthropic
