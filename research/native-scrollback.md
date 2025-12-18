# Native Terminal Scrollback with Ratatui

## Overview

Replace the current `tui-scrollview` based scrolling with native terminal scrollback using Ratatui's inline viewport and `insert_before()` API. This eliminates custom scroll state management and leverages the terminal's native scrollback buffer.

## Current Implementation (Problems)

The current approach in `src/app.rs` and `src/ui/chat.rs`:

1. **Alternate Screen Mode** - `EnterAlternateScreen` creates a separate buffer NOT part of native scrollback
2. **`tui-scrollview`** - Renders ALL content to a virtual buffer, clips to viewport
3. **Full re-render every frame** - Even off-screen turns are rendered
4. **Custom scroll state** - `ScrollViewState`, `scroll_up()`, `scroll_down()` etc.

**Performance issues:**
- O(entire conversation) rendering on every frame
- Markdown re-parsed for all visible turns during streaming
- Degrades with conversation length

## Target Architecture: Sliding Window

```
┌─────────────────────────────────────┐
│   Terminal Scrollback               │  ← Frozen, native scroll
│   (auto-committed lines)            │     Terminal manages this
│   - Line N-100                      │     User scrolls natively
│   - Line N-99                       │
│   - ...                             │
├─────────────────────────────────────┤
│   Hot Zone (viewport)               │  ← Our render target
│   - Line N-11                       │     Fixed size buffer
│   - Line N-10                       │     Re-render freely
│   - ...                             │     FIFO line management
│   - Line N (newest)                 │
│   ─────────────────────────────     │
│   > input box_                      │
└─────────────────────────────────────┘
```

**Key principle:** Lines flow through the hot zone. When new lines are added and the buffer overflows, the oldest lines are committed to scrollback via `insert_before(1, ...)` and become frozen.

## Ratatui Internals

### Viewport Modes

```rust
use ratatui::{Terminal, TerminalOptions, Viewport};

// Current (no native scrollback):
execute!(stdout, EnterAlternateScreen);
let terminal = Terminal::new(backend)?;

// Native scrollback:
let terminal = Terminal::with_options(
    backend,
    TerminalOptions {
        viewport: Viewport::Inline(height),  // height = hot zone size
    }
)?;
```

**Viewport types:**
- `Viewport::Fullscreen` - Default, uses alternate screen
- `Viewport::Inline(height)` - Fixed region at bottom, content above goes to scrollback
- `Viewport::Fixed(Rect)` - Fixed position anywhere on screen

### The `insert_before()` API

```rust
pub fn insert_before<F>(&mut self, height: u16, draw_fn: F) -> Result<()>
where
    F: FnOnce(&mut Buffer)
```

**Behavior:**
1. Creates a temporary `Buffer` of `height` lines
2. Calls `draw_fn` to render content into the buffer
3. Inserts the buffer content ABOVE the viewport
4. If viewport not at screen bottom → pushes viewport down
5. If viewport at bottom → scrolls content above upward
6. Excess content goes into terminal's native scrollback buffer

**Example:**
```rust
terminal.insert_before(1, |buf| {
    Paragraph::new(line).render(buf.area, buf);
})?;
```

### Backend Selection

**Recommendation: Crossterm** (already in use)

| Backend | Windows | Linux/Mac | Scrolling Regions |
|---------|---------|-----------|-------------------|
| Crossterm | ✅ (Win10+) | ✅ | ✅ |
| Termion | ❌ | ✅ | ✅ |
| Termwiz | ✅ | ✅ | ✅ |

Crossterm is the most popular and best documented. All backends support scrolling regions when the feature is enabled.

## Tearing Prevention

There are **two separate mechanisms** for preventing visual tearing:

### 1. `scrolling-regions` Feature (for `insert_before()`)

Enable flicker-free scrollback promotion:

```toml
# Cargo.toml
ratatui = { version = "0.30.0-beta.0", features = ["scrolling-regions"] }
```

This uses ANSI escape sequences (`^[[X;Yr`) to create scroll regions, allowing smooth content insertion without full-screen redraws. **Required for this implementation.**

**Backend methods added:**
- `scroll_region_up(region: Range<u16>, line_count: u16)`
- `scroll_region_down(region: Range<u16>, line_count: u16)`

### 2. `BeginSynchronizedUpdate` (for `terminal.draw()`)

Prevents tearing during viewport redraws by buffering changes:

```rust
queue!(backend, BeginSynchronizedUpdate)?;
// All rendering here is buffered
terminal.draw(...)?;
queue!(backend, EndSynchronizedUpdate)?;
backend.flush()?;
```

**You probably don't need this** because:
- Ratatui's double-buffering already minimizes redraws (only changed cells are written)
- `scrolling-regions` handles the `insert_before()` case
- Synchronized updates add latency

**When you might need it:**
- If you see tearing during fast streaming updates
- If terminal doesn't fully support scrolling regions

### Optimal Configuration

```toml
# Cargo.toml - enable scrolling regions, remove synchronized updates
ratatui = { version = "0.30.0-beta.0", features = ["scrolling-regions"] }
```

```rust
// Don't use BeginSynchronizedUpdate unless you observe tearing
fn draw_viewport(&mut self) -> Result<()> {
    self.terminal.draw(|frame| {
        // ... render widgets
    })?;
    Ok(())
}
```

Test without synchronized updates first. The `scrolling-regions` feature was specifically designed to eliminate the flickering that `insert_before()` used to cause.

## Implementation Plan

### 1. Create Hot Zone Structure

The hot zone is a sliding window that tracks:
- Lines currently visible in the viewport (re-renderable)
- How many lines from active turns have been committed to scrollback
- Which turns are "frozen" (fully committed, never re-render)

```rust
use std::collections::{VecDeque, HashSet};
use ratatui::text::Line;

pub struct HotZone {
    /// Lines currently in the re-renderable hot zone
    lines: VecDeque<Line<'static>>,
    /// Maximum lines before overflow commits to scrollback
    max_lines: usize,
    /// Lines committed from ACTIVE turns (not frozen ones)
    committed_count: usize,
    /// Turn IDs fully committed to scrollback - never re-render these
    frozen_turn_ids: HashSet<usize>,
}

impl HotZone {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(max_lines),
            max_lines,
            committed_count: 0,
            frozen_turn_ids: HashSet::new(),
        }
    }

    /// Render only active (non-frozen) turns.
    /// Overflow lines promote to scrollback automatically.
    pub fn render_active_turns<B: Backend>(
        &mut self,
        transcript: &Transcript,
        width: u16,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
        // Only render turns NOT in frozen set - avoids re-rendering
        // entire conversation history
        let active_lines: Vec<Line<'static>> = transcript
            .turns()
            .filter(|t| !self.frozen_turn_ids.contains(&t.id))
            .flat_map(|t| t.render(width).into_iter().map(|l| l.into_owned()))
            .collect();

        // Skip lines already committed to scrollback
        let hot_lines: Vec<_> = active_lines
            .into_iter()
            .skip(self.committed_count)
            .collect();

        self.lines.clear();

        for line in hot_lines {
            self.lines.push_back(line);

            // Overflow promotes to scrollback
            while self.lines.len() > self.max_lines {
                let committed = self.lines.pop_front().unwrap();
                terminal.insert_before(1, |buf| {
                    Paragraph::new(committed).render(buf.area, buf);
                })?;
                self.committed_count += 1;
            }
        }

        Ok(())
    }

    /// Mark a turn as frozen when all its lines are in scrollback.
    /// Resets committed_count since frozen turns leave the active set.
    pub fn freeze_turn(&mut self, turn_id: usize, turn_line_count: usize) {
        self.frozen_turn_ids.insert(turn_id);
        // Adjust committed_count: subtract the frozen turn's lines
        self.committed_count = self.committed_count.saturating_sub(turn_line_count);
    }

    /// Check if a turn should be frozen (all lines committed)
    pub fn should_freeze_turn(&self, turn_line_count: usize) -> bool {
        self.committed_count >= turn_line_count
    }

    /// Get current lines for viewport rendering
    pub fn lines(&self) -> &VecDeque<Line<'static>> {
        &self.lines
    }

    /// Clear everything (e.g., new session)
    pub fn reset(&mut self) {
        self.lines.clear();
        self.committed_count = 0;
        self.frozen_turn_ids.clear();
    }
}
```

### Key Concepts

**Frozen vs Active Turns:**
- **Active turns**: Have content in the hot zone, get re-rendered on each frame
- **Frozen turns**: Fully committed to scrollback, never re-rendered

**Why this matters:**
- Avoids O(entire conversation) re-rendering
- Only active turns (usually 1-2) are processed each frame
- `committed_count` tracks position within active turns only

**The flow for a streaming response:**
```
1. User submits message
   → User turn rendered into hot zone
   → If overflows, lines promote to scrollback

2. Assistant streams tokens
   → Re-render active turns (user + assistant)
   → Skip first `committed_count` lines (already in scrollback)
   → Overflow promotes, committed_count increments

3. User turn fully scrolls out
   → All user turn lines now in scrollback
   → freeze_turn(user_turn_id) - never re-render it again
   → committed_count adjusts

4. Assistant continues streaming
   → Only assistant turn is re-rendered now
   → Process continues...

5. Turn ends
   → Hot zone still has last N lines visible
   → Next interaction starts, old content naturally scrolls up
```

### 2. Modify Terminal Setup

```rust
// src/app.rs

pub async fn new(config: Config, continue_session: bool) -> Result<Self> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();

    // No EnterAlternateScreen - we want native scrollback
    // No EnableMouseCapture - terminal handles scroll natively
    execute!(
        stdout,
        crossterm::terminal::SetTitle(...),
    )?;

    let backend = CrosstermBackend::new(stdout);
    let terminal_size = crossterm::terminal::size()?;

    // Use inline viewport - hot zone is full terminal height
    let terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(terminal_size.1),
        }
    )?;

    let hot_zone = HotZone::new(terminal_size.1 as usize);

    // ...
}
```

### 3. Modify Rendering Pipeline

```rust
// In your App struct
struct App {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    hot_zone: HotZone,
    transcript: Transcript,
    input: InputBox,
}

impl App {
    async fn stream_response(&mut self, agent: &mut Agent, prompt: &str) -> Result<()> {
        let mut stream = agent.process_message(prompt);

        loop {
            let step = stream.next().await;

            match step {
                Some(AgentStep::TextDelta(text)) => {
                    let turn = self.transcript.get_or_create_current_turn();
                    turn.append_to_active(&text);

                    // Re-render active turns, overflow promotes to scrollback
                    let width = self.terminal.size()?.width;
                    self.hot_zone.render_active_turns(
                        &self.transcript,
                        width,
                        &mut self.terminal,
                    )?;

                    // Check if any active turns should be frozen
                    self.check_freeze_turns(width);

                    // Redraw viewport
                    self.draw_viewport()?;
                }

                Some(AgentStep::Finished { .. }) => {
                    // Nothing special - hot zone keeps last N lines
                    // Next turn will naturally push old content up
                    break;
                }

                None => break,
                _ => { /* handle other steps */ }
            }
        }

        Ok(())
    }

    /// Check if any active turns have fully scrolled into scrollback
    fn check_freeze_turns(&mut self, width: u16) {
        let mut to_freeze = Vec::new();

        for turn in self.transcript.turns() {
            if self.hot_zone.frozen_turn_ids.contains(&turn.id) {
                continue;
            }

            let line_count = turn.render(width).len();

            if self.hot_zone.should_freeze_turn(line_count) {
                to_freeze.push((turn.id, line_count));
            } else {
                // Once we hit a turn that's not fully committed, stop
                // (turns are ordered, later turns can't be frozen if earlier ones aren't)
                break;
            }
        }

        for (turn_id, line_count) in to_freeze {
            self.hot_zone.freeze_turn(turn_id, line_count);
        }
    }

    fn draw_viewport(&mut self) -> Result<()> {
        self.terminal.draw(|frame| {
            let area = frame.area();

            let chunks = Layout::vertical([
                Constraint::Min(1),        // Hot zone content
                Constraint::Length(5),     // Input box
            ]).split(area);

            // Render hot zone lines
            let lines: Vec<Line> = self.hot_zone.lines().iter().cloned().collect();
            frame.render_widget(Paragraph::new(lines), chunks[0]);

            // Render input
            frame.render_widget(self.input.widget(), chunks[1]);
        })?;

        Ok(())
    }
}
```

### 4. Session Restore

No special handling needed. On startup, existing turns render through the hot zone like normal - overflow naturally promotes to scrollback:

```rust
pub async fn run(&mut self) -> Result<()> {
    // Initial render pushes existing content through hot zone
    // Overflow promotes to scrollback automatically
    let width = self.terminal.size()?.width;
    self.hot_zone.render_active_turns(&self.transcript, width, &mut self.terminal)?;
    self.draw_viewport()?;

    // Main loop continues normally
    // ...
}
```

The hot zone handles everything uniformly - no distinction between "restoring" vs "streaming".

### 5. Cleanup Changes

```rust
fn cleanup(&mut self) -> Result<()> {
    disable_raw_mode()?;
    // No LeaveAlternateScreen - we never entered it
    // No DisableMouseCapture - we never enabled it
    self.terminal.show_cursor()?;
    Ok(())
}
```

## Files to Modify

| File | Changes |
|------|---------|
| `src/app.rs` | Remove `EnterAlternateScreen`, use `Viewport::Inline`, add `HotZone`, change render pipeline |
| `src/ui/chat.rs` | Replace with simpler `ViewportWidget`, remove `ScrollViewState` |
| `src/ui/mod.rs` | Update exports |
| `Cargo.toml` | Add `features = ["scrolling-regions"]`, remove `tui-scrollview` |

## What Gets Removed

- `tui-scrollview` dependency
- `ChatView` struct with scroll state
- `ScrollViewState`
- `scroll_up()`, `scroll_down()`, `page_up()`, `page_down()` methods
- `auto_scroll` logic
- `EnterAlternateScreen` / `LeaveAlternateScreen`
- `EnableMouseCapture` / `DisableMouseCapture` - terminal handles scroll natively
- Mouse event handling (`MouseEventKind::ScrollUp`, `ScrollDown`)
- `map_mouse()` function and related action mappings

## Edge Cases to Resolve

### 1. Blocks Larger Than Hot Zone

If a single turn/block renders to more lines than the hot zone size:
- Lines at the top get committed before the block finishes rendering
- Subsequent markdown re-parsing can't fix already-committed lines

**Mitigation:**
- Accept minor rendering glitches in scrollback for very long blocks
- Most markdown stabilizes quickly (code fences close, lists complete)
- Hot zone of 12+ lines covers most partial-render cases

### 2. Terminal Resize

When terminal resizes:
- Hot zone `max_lines` should update
- Already-committed content in scrollback may have wrong width
- Content in hot zone needs re-render at new width

**Solution:**
```rust
Event::Resize(width, height) => {
    self.hot_zone.set_max_lines(height);
    // Re-render current content at new width
    self.rerender_hot_zone(width);
}
```

### 3. Mouse Events Removed

With native scrollback, we remove mouse capture entirely:
- Terminal handles scroll wheel natively
- No need for `EnableMouseCapture` / `DisableMouseCapture`
- No need for `map_mouse()` or scroll action handling

**Trade-off:** We lose any other mouse functionality (click-to-position, selection, etc.) but this is acceptable for a keyboard-first TUI.

### 4. Content Width in `insert_before()`

The buffer provided to `insert_before()` is screen-width. Long lines may need wrapping.

**Note:** There's an open issue ([#1426](https://github.com/ratatui/ratatui/issues/1426)) for `insert_lines_before()` that would handle this better by delegating wrapping to the terminal.

**Current solution:** Pre-wrap lines to terminal width before committing.

### 5. Input Box Position

Input box should always be at the bottom of the viewport, below the hot zone content.

**Solution:** Reserve fixed space in viewport layout:
```rust
Layout::vertical([
    Constraint::Min(1),        // Hot zone (flexible)
    Constraint::Length(5),     // Input (fixed)
])
```

### 6. Tearing During Fast Updates

The `scrolling-regions` feature handles `insert_before()` flicker. If you still see tearing:

1. **First**: Ensure `scrolling-regions` feature is enabled
2. **Second**: Try adding synchronized updates as fallback:

```rust
fn render_frame(&mut self) -> Result<()> {
    queue!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;
    self.hot_zone.render_active_turns(...)?;
    self.draw_viewport()?;
    queue!(self.terminal.backend_mut(), EndSynchronizedUpdate)?;
    self.terminal.backend_mut().flush()?;
    Ok(())
}
```

**Note**: Synchronized updates add latency - only use if needed.

### 7. Session Restore Performance

Large transcripts will push many lines to scrollback on initial render. This happens once at startup via the normal hot zone overflow mechanism - no special handling needed.

If slow, the existing line-by-line promotion is already optimal since each `insert_before(1, ...)` is a single scroll operation.

### 8. Turn/Block Boundaries in Scrollback

Currently, turns have visual separators and headers. These need to be included when committing lines.

**Solution:** Render turn headers and separators as regular lines in the hot zone, they'll naturally commit with content.

## Performance Characteristics

| Aspect | Before (tui-scrollview) | After (native scrollback) |
|--------|------------------------|---------------------------|
| Render complexity | O(all turns) | O(active turns only, typically 1-2) |
| Memory for scroll | Full virtual buffer | Hot zone only (viewport height) |
| Scroll performance | Custom, can lag | Native, instant |
| Copy/paste | Custom selection | Native terminal |
| Markdown re-render | All visible turns | Active turns only |
| Frozen content | N/A | Never re-rendered, zero cost |

## References

- [Ratatui Terminal Documentation](https://docs.rs/ratatui/latest/ratatui/struct.Terminal.html)
- [Inline Viewport Example](https://ratatui.rs/examples/apps/inline/)
- [v0.29.0 Release - Scrolling Regions](https://ratatui.rs/highlights/v029/)
- [GitHub PR #1341 - Scrolling Regions Implementation](https://github.com/ratatui/ratatui/pull/1341)
- [GitHub Issue #1426 - insert_lines_before](https://github.com/ratatui/ratatui/issues/1426)
- [GitHub Issue #2077 - Scrolling regions feature flag](https://github.com/ratatui/ratatui/issues/2077)
- [Backend Comparison](https://ratatui.rs/concepts/backends/comparison/)
- [BeginSynchronizedUpdate docs](https://docs.rs/crossterm/latest/crossterm/terminal/struct.BeginSynchronizedUpdate.html)
