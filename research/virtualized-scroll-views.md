# Virtualized Scroll Views for Ratatui

## Problem

The current `ChatView` uses `tui-scrollview` which renders ALL content to a virtual buffer, then clips to the viewport. This means:
- Every turn is rendered every frame, even if off-screen
- During streaming, markdown is re-parsed on every token
- Scrolling while streaming is sluggish
- Performance degrades with conversation length

## Current Implementation

```rust
// src/ui/chat.rs - current approach
let mut scroll_view = ScrollView::new(Size::new(content_width, content_height));

// Renders EVERYTHING
for turn in transcript.turns() {
    let (header, content) = render_turn(turn, width);
    scroll_view.render_widget(header, ...);
    scroll_view.render_widget(content, ...);
}

// Then clips to viewport
scroll_view.render(area, buf, &mut scroll_state);
```

## Investigated Solutions

### 1. tui-widget-list (v0.13.3)

**Repo:** https://github.com/preiter93/tui-widget-list

**How it works:**
- Uses a `ListBuilder` closure that's only called for visible items
- Returns `(widget, height)` for each item
- True virtualization - invisible items never constructed

```rust
let builder = ListBuilder::new(|context| {
    let turn = &turns[context.index];
    let widget = TurnWidget::new(turn, width);
    let height = widget.height();
    (widget, height)
});

let list = ListView::new(builder, item_count)
    .scroll_padding(1)
    .infinite_scrolling(false);

StatefulWidget::render(list, area, buf, &mut state);
```

**Pros:**
- True virtualization
- Variable height items supported
- Active maintenance
- Has scroll_padding for auto-scroll behavior

**Cons:**
- ⚠️ Depends on ratatui 0.29, we use 0.30-beta
- Item-based scrolling (next/previous), not line-based
- Would need patching to work with our ratatui version

**API Notes:**
- `ListState` uses `next()` / `previous()` not scroll_up/down
- `scroll_offset_index()` - first visible item index
- `scroll_truncation()` - rows of first item hidden above viewport

### 2. rat-scrolled (v1.5.0)

**Repo:** https://github.com/thscharler/rat-salsa/tree/master/rat-scrolled

Part of the `rat-salsa` framework. Lower-level scroll infrastructure.

**How it works:**
- Provides `Scroll`, `ScrollArea`, `ScrollState` primitives
- Widget handles its own scrolling internally using offset
- You implement what to render based on offset

**Pros:**
- More control
- Part of larger framework with other widgets

**Cons:**
- More work to implement
- Need to calculate visible items yourself
- Part of larger framework (more dependencies)

### 3. Manual Virtualization (DIY)

Implement ourselves without external crates:

```rust
struct CachedTurn {
    turn_id: usize,
    height: u16,           // Always calculated
    content: Option<Paragraph<'static>>,  // Only for terminal turns
}

impl Widget for ChatViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // 1. Calculate heights for all turns (cheap for terminal, cached)
        // 2. Find first visible turn based on scroll offset
        // 3. Only render turns that intersect viewport
        // 4. Use cached content for terminal turns
    }
}
```

**Pros:**
- No dependency issues
- Full control over behavior
- Can combine with caching

**Cons:**
- More code to maintain
- Need to handle edge cases (partial visibility, etc.)

## Recommended Approach

### Short-term: Height Caching + Skip Rendering

Without changing scroll crate, we can still optimize:

1. **Cache heights** for terminal turns (content won't change)
2. **Calculate cumulative Y positions** from cached heights
3. **Determine visible range** based on scroll offset
4. **Only call `block.render()`** for visible turns

This gives us the virtualization benefit while keeping `tui-scrollview`.

### Long-term: Fork/Patch tui-widget-list

When we have time:
1. Create `lib/patches/tui-widget-list/`
2. Update its `Cargo.toml` to use ratatui 0.30
3. Fix any breaking API changes
4. Use patched version

The crate is small (~700 lines) so patching should be manageable.

## Key Insight: Terminal vs Streaming Turns

The fundamental optimization is distinguishing:
- **Terminal turns**: Content frozen, can cache everything (height + rendered content)
- **Streaming turns**: Must re-render each frame, but there's only 1-2 of these at a time

Even without virtualization, caching terminal turn content would help significantly.

## Files to Modify

| File | Changes Needed |
|------|----------------|
| `src/ui/chat.rs` | Add height cache, visible range calculation |
| `src/transcript.rs` | `Turn::is_terminal()` already exists |
| `Cargo.toml` | Add patched tui-widget-list when ready |

## Related Performance Issues

1. **Markdown re-parsing** - ratskin parses on every render
2. **ThinkingBlock styling** - clones lines for `patch_style()`
3. **Line conversion** - need `to_owned_line()` helper for static lifetimes

## References

- tui-widget-list docs: https://docs.rs/tui-widget-list/
- rat-scrolled docs: https://docs.rs/rat-scrolled/
- tui-scrollview (current): https://docs.rs/tui-scrollview/
