# Browser Sessions: Persistent Interactive Browser for Agents

## Problem

The current `fetch_html` tool is fire-and-forget: it launches a browser, waits a
fixed 10s timer, grabs the rendered HTML, runs readability extraction, and tears
everything down. This means:

1. **No interaction** — can't click, fill forms, navigate, or scroll.
2. **No session reuse** — every fetch pays the full browser startup cost.
3. **Coarse timing** — a fixed `page_load_wait_ms` timer either wastes time on
   fast pages or misses content on slow SPAs.
4. **No state continuity** — can't log in and then navigate authenticated pages
   across multiple tool calls.

## Design

### Architecture

Browser sessions are managed in-process by a `BrowserSessionManager` that holds
a `HashMap<String, BrowserSession>`. Each `BrowserSession` owns a `Browser`
handle (from chromiumoxide) and its active `Page`. The chromiumoxide event
handler task runs in a background tokio task per session.

There is no separate daemon process — the Chromium child process is already
external, and we communicate with it via CDP (Chrome DevTools Protocol) through
chromiumoxide. The session manager just keeps the handles alive between tool
calls.

### Page Load Strategy

Replace the fixed timer with **CDP lifecycle events**:

1. Wait for `Page.loadEventFired` (DOM ready + subresources loaded)
2. Then wait for network idle: poll `Page.lifecycleEvent` for `networkIdle`,
   or fall back to a short debounce (500ms with no new network requests)
3. Cap with a configurable timeout (default 30s) so slow pages don't block
   forever

This is both faster (no wasted 10s on fast pages) and more reliable (catches
late-loading SPA content).

### Content Extraction Modes

The agent chooses the extraction format per-request:

- **`readability`** — Current behavior. Runs readability algorithm, converts to
  markdown. Best for articles, documentation, blog posts.
- **`accessibility`** — Traverses the accessibility tree via CDP's
  `Accessibility.getFullAXTree`. Returns a compact representation of interactive
  elements with deterministic refs (`@e1`, `@e2`, ...). Best for forms, UIs,
  dashboards. (Phase 2 — not in initial implementation.)

### Session Lifecycle

- Agent opens a session, gets back a session ID + initial page content
- Agent performs actions (click, fill, navigate) on the session by name
- Agent requests snapshots (readability content) at any point
- Agent explicitly closes the session when done
- Safety net: sessions auto-close after 10 minutes idle

### Tool Surface

| Tool | Params | Returns |
|---|---|---|
| `browser_open` | `url`, `session_name?` | session_name, page title, readability content |
| `browser_action` | `session_name`, `action` (navigate/click/fill/scroll/back/forward/wait), action-specific params | updated readability content |
| `browser_snapshot` | `session_name`, `format?` (readability) | content in requested format |
| `browser_list_sessions` | — | list of active sessions with URLs |
| `browser_close` | `session_name` | confirmation |

### Action Types for `browser_action`

- **`navigate`** `{url}` — Navigate to a new URL
- **`click`** `{selector}` — Click an element by CSS selector
- **`fill`** `{selector, value}` — Type into an input field
- **`select`** `{selector, value}` — Select an option from a dropdown
- **`scroll`** `{direction, amount?}` — Scroll the page (up/down/to_element)
- **`back`** / **`forward`** — Browser history navigation
- **`wait`** `{ms}` — Explicit wait (escape hatch for tricky pages)
- **`evaluate`** `{script}` — Run JavaScript and return result

All actions block until the page settles (network idle) before returning
content, so the agent always gets a consistent snapshot.

## Implementation Plan

### Files to Create/Modify

```
src/tools/browser/mod.rs          — Add BrowserSessionManager, BrowserSession
src/tools/browser/session.rs      — New: session manager implementation
src/tools/impls/browser_session.rs — New: tool definitions (open/action/snapshot/list/close)
src/tools/impls/mod.rs            — Register new tools
src/tools/mod.rs                  — Add tool names, exports, registry entries
src/tools/handlers.rs             — Add effect handlers
src/effect.rs                     — Add Effect variants
src/app.rs                        — Handle new effects
```

### Phase 1 (This PR)

1. **BrowserSessionManager** — Manages named sessions with idle timeout
2. **browser_open** — Opens a URL in a persistent session, blocks until loaded,
   returns readability content
3. **browser_action** — Performs actions on an existing session
4. **browser_snapshot** — Returns current page content (readability format)
5. **browser_list_sessions** — Lists active sessions
6. **browser_close** — Closes a session

### Phase 2 (Future)

- Accessibility tree snapshots with `@ref` addressing
- Screenshot capture tool
- Cookie/storage inspection
- HAR recording
- Profile persistence across sessions

## Conventions

- Tool names: `mcp_browser_open`, `mcp_browser_action`, etc.
- Effect variants: `BrowserOpen`, `BrowserAction`, `BrowserSnapshot`,
  `BrowserListSessions`, `BrowserClose`
- Follows existing `list_/get_` pattern from background_tasks and agent_management
- All tools require user approval
- All tools support `background` parameter for non-blocking execution

## Inspiration

Informed by [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser),
which uses a similar persistent session + snapshot model but as a separate
TypeScript daemon + Rust CLI. Our approach is simpler: in-process session
management, no IPC, direct CDP access via chromiumoxide.
