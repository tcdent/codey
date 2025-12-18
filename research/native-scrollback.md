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

### Scrolling Regions Feature (v0.29+)

Enable flicker-free `insert_before()`:

```toml
# Cargo.toml
ratatui = { version = "0.30.0-beta.0", features = ["scrolling-regions"] }
```

This uses ANSI escape sequences (`^[[X;Yr`) to create scroll regions, allowing smooth content insertion without full-screen redraws.

**Backend methods added:**
- `scroll_region_up(region: Range<u16>, line_count: u16)`
- `scroll_region_down(region: Range<u16>, line_count: u16)`

## Implementation Plan

### 1. Create Line Buffer Structure

```rust
use std::collections::VecDeque;
use ratatui::text::Line;

pub struct HotZone {
    /// Lines currently in the re-renderable hot zone
    lines: VecDeque<Line<'static>>,
    /// Maximum lines before overflow commits to scrollback
    max_lines: u16,
}

impl HotZone {
    pub fn new(max_lines: u16) -> Self {
        Self {
            lines: VecDeque::new(),
            max_lines,
        }
    }

    /// Push a line, committing overflow to scrollback
    pub fn push_line(
        &mut self,
        line: Line<'static>,
        terminal: &mut Terminal<impl Backend>,
    ) -> Result<()> {
        self.lines.push_back(line);
        self.commit_overflow(terminal)
    }

    /// Push multiple lines
    pub fn push_lines(
        &mut self,
        lines: Vec<Line<'static>>,
        terminal: &mut Terminal<impl Backend>,
    ) -> Result<()> {
        for line in lines {
            self.lines.push_back(line);
        }
        self.commit_overflow(terminal)
    }

    /// Commit overflowing lines to scrollback
    fn commit_overflow(
        &mut self,
        terminal: &mut Terminal<impl Backend>,
    ) -> Result<()> {
        while self.lines.len() > self.max_lines as usize {
            let committed = self.lines.pop_front().unwrap();
            terminal.insert_before(1, |buf| {
                Paragraph::new(committed).render(buf.area, buf);
            })?;
        }
        Ok(())
    }

    /// Re-render the last N lines (for markdown updates)
    pub fn rerender_tail(&mut self, count: usize, new_lines: Vec<Line<'static>>) {
        // Remove old tail
        for _ in 0..count.min(self.lines.len()) {
            self.lines.pop_back();
        }
        // Add new rendering
        for line in new_lines {
            self.lines.push_back(line);
        }
    }

    /// Get current lines for viewport rendering
    pub fn lines(&self) -> impl Iterator<Item = &Line<'static>> {
        self.lines.iter()
    }

    /// Current line count
    pub fn len(&self) -> usize {
        self.lines.len()
    }
}
```

### 2. Modify Terminal Setup

```rust
// src/app.rs

pub async fn new(config: Config, continue_session: bool) -> Result<Self> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();

    // DON'T enter alternate screen
    execute!(
        stdout,
        EnableMouseCapture,  // May need to disable for native scroll
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

    let hot_zone = HotZone::new(terminal_size.1);

    // ...
}
```

### 3. Modify Rendering Pipeline

```rust
// During streaming, convert Turn/Block output to lines and push to hot zone
AgentStep::TextDelta(text) => {
    let turn = self.transcript.get_or_create_current_turn();
    turn.append_to_active(&text);

    // Re-render the current block and update hot zone
    let width = self.terminal.size()?.width;
    let rendered_lines = turn.render(width);

    // Calculate how many lines changed (for efficient rerender)
    let new_line_count = rendered_lines.len();
    let prev_line_count = self.prev_render_line_count;

    // Rerender the tail that may have changed
    self.hot_zone.rerender_tail(prev_line_count, rendered_lines);
    self.prev_render_line_count = new_line_count;
}
```

### 4. Simplify Viewport Widget

```rust
// src/ui/viewport.rs (replaces complex chat.rs)

pub struct ViewportWidget<'a> {
    hot_zone: &'a HotZone,
    input: &'a InputBox,
}

impl Widget for ViewportWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::vertical([
            Constraint::Min(1),           // Hot zone content
            Constraint::Length(5),        // Input box
        ]).split(area);

        // Render hot zone lines
        let lines: Vec<Line> = self.hot_zone.lines().cloned().collect();
        Paragraph::new(lines).render(chunks[0], buf);

        // Render input
        self.input.widget().render(chunks[1], buf);
    }
}
```

### 5. Session Restore

```rust
pub async fn run(&mut self) -> Result<()> {
    // On startup, push all existing transcript content to scrollback
    if self.continue_session {
        for turn in self.transcript.turns() {
            let lines = turn.render(self.terminal.size()?.width);
            for line in lines {
                self.terminal.insert_before(1, |buf| {
                    Paragraph::new(line).render(buf.area, buf);
                })?;
            }
        }
    }

    // Main loop with empty hot zone
    // ...
}
```

### 6. Cleanup Changes

```rust
fn cleanup(&mut self) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        self.terminal.backend_mut(),
        // NO LeaveAlternateScreen - we never entered it
        DisableMouseCapture,
    )?;
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

### 3. Mouse Capture vs Native Scroll

With mouse capture enabled:
- Terminal can't use mouse wheel for native scrollback
- Must disable mouse capture OR handle scroll events manually

**Options:**
- Disable `EnableMouseCapture` entirely (lose mouse input)
- Capture mouse but forward scroll events (complex)
- Hybrid: disable capture when not streaming (user can scroll natively)

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

### 6. Synchronized Updates During Commit

When committing lines to scrollback while also updating viewport:
- Use `BeginSynchronizedUpdate` / `EndSynchronizedUpdate`
- Prevents visual tearing during the push

### 7. Session Restore Performance

For large transcripts, pushing all lines via `insert_before(1, ...)` one at a time may be slow.

**Optimization:** Batch into larger chunks:
```rust
// Instead of line-by-line:
terminal.insert_before(chunk.len(), |buf| {
    Paragraph::new(chunk).render(buf.area, buf);
})?;
```

### 8. Turn/Block Boundaries in Scrollback

Currently, turns have visual separators and headers. These need to be included when committing lines.

**Solution:** Render turn headers and separators as regular lines in the hot zone, they'll naturally commit with content.

## Performance Characteristics

| Aspect | Before (tui-scrollview) | After (native scrollback) |
|--------|------------------------|---------------------------|
| Render complexity | O(all turns) | O(hot zone lines) |
| Memory for scroll | Full virtual buffer | Hot zone only (~12 lines) |
| Scroll performance | Custom, can lag | Native, instant |
| Copy/paste | Custom selection | Native terminal |
| Markdown re-render | All visible turns | Hot zone tail only |

## References

- [Ratatui Terminal Documentation](https://docs.rs/ratatui/latest/ratatui/struct.Terminal.html)
- [Inline Viewport Example](https://ratatui.rs/examples/apps/inline/)
- [v0.29.0 Release - Scrolling Regions](https://ratatui.rs/highlights/v029/)
- [GitHub Issue #1426 - insert_lines_before](https://github.com/ratatui/ratatui/issues/1426)
- [GitHub Issue #2077 - Scrolling regions feature flag](https://github.com/ratatui/ratatui/issues/2077)
