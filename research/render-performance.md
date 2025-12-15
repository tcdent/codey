# Render Performance Research

This document captures research, implementation decisions, and future enhancement opportunities for optimizing terminal rendering performance in Codey, specifically targeting Ghostty.

## Table of Contents

1. [Context & Goals](#context--goals)
2. [Architecture Analysis](#architecture-analysis)
3. [Implemented Optimizations](#implemented-optimizations)
4. [Future Enhancements](#future-enhancements)
5. [SIMD Diff Implementation](#simd-diff-implementation)
6. [References](#references)

---

## Context & Goals

Codey runs as a terminal UI application using ratatui + crossterm, primarily in Ghostty. The goals of this research were:

1. **Cap refresh rate to 60fps** - Prevent excessive redraws during streaming
2. **Event-driven rendering** - Only render when state actually changes
3. **Explore GPU offloading** - Leverage Ghostty's GPU acceleration

---

## Architecture Analysis

### Current Stack

```
┌─────────────────────────────────────────────┐
│  Codey Application                          │
│  └── Widgets (ChatView, InputBox)           │
├─────────────────────────────────────────────┤
│  ratatui (v0.30.0-beta.0)                   │
│  └── Terminal, Buffer, Frame                │
│  └── Widget trait, Layout                   │
├─────────────────────────────────────────────┤
│  ratatui-core                               │
│  └── Buffer::diff() ← performance hotspot   │
│  └── Cell storage (Vec<Cell>)               │
├─────────────────────────────────────────────┤
│  crossterm (v0.28)                          │
│  └── ANSI escape sequence generation        │
│  └── Terminal control (raw mode, etc.)      │
├─────────────────────────────────────────────┤
│  Ghostty Terminal Emulator                  │
│  └── GPU-accelerated rendering (Metal/GL)   │
│  └── SIMD-optimized parsing                 │
└─────────────────────────────────────────────┘
```

### Rendering Flow

```
1. Widget::render() → writes to Buffer
2. Terminal::draw() → calls widget render in closure
3. Terminal::flush() → calls Buffer::diff(prev, curr)
4. Buffer::diff() → returns Vec<(x, y, &Cell)> of changes
5. Backend::draw() → generates ANSI sequences for changes
6. Write to stdout → Ghostty parses and renders
```

### Ghostty's Optimizations (for reference)

Ghostty achieves high performance through:

| Technique | Implementation | Speedup |
|-----------|---------------|---------|
| SIMD UTF-8 decode | `simd.vt.utf8DecodeUntilControlSeq()` | 7-16x |
| CSI fast-path parser | Optimistic parsing for common sequences | 1.4-2x |
| Codepoint width tables | 3-stage trie lookup | 2.8-5x |
| Grapheme break tables | Sub-1KB lookup table | 8x |
| IO/Render thread split | Dedicated threads with mutex | 60 FPS |
| Page-based cell storage | Contiguous memory, no reallocation | O(1) scroll |

### ratatui's Current Approach

| Component | Implementation | Limitation |
|-----------|---------------|------------|
| Cell storage | `Vec<Cell>` with CompactString | Memory fragmentation |
| Diff algorithm | Sequential O(n) cell comparison | No SIMD, no batching |
| Style tracking | Per-cell Style struct | Redundant comparisons |
| Width calculation | `unicode_width` crate per render | No caching |

---

## Implemented Optimizations

### 1. Synchronized Updates

**Problem:** Rapid updates during streaming cause visual tearing.

**Solution:** Wrap draw calls with terminal synchronization:

```rust
// Begin synchronized update - terminal buffers all changes
queue!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;

self.terminal.draw(|frame| { /* render widgets */ })?;

// End synchronized update - terminal renders atomically
queue!(self.terminal.backend_mut(), EndSynchronizedUpdate)?;
self.terminal.backend_mut().flush()?;
```

**How it works:** Uses CSI `? 2026 h` / `? 2026 l` sequences. Ghostty (and other modern terminals) buffer all output between these markers and render atomically.

**Impact:** Eliminates tearing, feels snappier.

### 2. Event-Driven Rendering

**Problem:** Previous implementation used a `dirty` flag that polled every 50-500ms.

**Solution:** Removed the dirty flag entirely. Now:

```rust
// Block until we get an event - no polling when idle
if event::poll(Duration::from_secs(60))? {
    let needs_redraw = match event::read()? {
        Event::Key(key) => { self.handle_key_event(key); true }
        Event::Mouse(mouse) => { self.handle_mouse_event(mouse); true }
        Event::Resize(_, _) => true,
        _ => false,
    };

    if needs_redraw {
        self.draw()?;
    }
}
```

**Impact:** Zero CPU usage when idle. Only renders on actual input.

### 3. Frame Rate Limiting (60fps cap)

**Problem:** During LLM streaming, `TextDelta` events arrive rapidly (potentially 100+/sec), each triggering a redraw.

**Solution:** Throttled drawing for streaming:

```rust
const MIN_FRAME_TIME: Duration = Duration::from_millis(16); // ~60fps

fn draw_throttled(&mut self) -> Result<bool> {
    if self.last_render.elapsed() >= MIN_FRAME_TIME {
        self.draw()?;
        Ok(true)
    } else {
        Ok(false)
    }
}
```

Used for `TextDelta` events:
```rust
AgentStep::TextDelta(text) => {
    // ... update transcript ...
    self.draw_throttled()?; // Caps at 60fps
}
```

Final draw after streaming ensures complete state:
```rust
// Final draw to ensure complete state is rendered
self.draw()?;
```

**Impact:** Smooth 60fps during streaming, reduced CPU usage.

### 4. Skip Flag (Automatic)

**Status:** Already handled by ratatui's diff algorithm. The `Buffer::diff()` method compares cells and only returns changes. No manual intervention needed for basic cases.

The `Cell.skip` flag is for special cases (image protocols) where cells should be excluded from diffing entirely - not needed for our use case.

---

## Future Enhancements

### Tier 1: Low Effort, High Impact

| Enhancement | Description | Effort |
|-------------|-------------|--------|
| Batched ANSI output | Use crossterm's `queue!` for all operations | Low |
| Combined color sequences | `SetColors` instead of separate fg/bg | Low |
| Style run encoding | Group consecutive same-style cells | Medium |

### Tier 2: Medium Effort, High Impact

| Enhancement | Description | Effort |
|-------------|-------------|--------|
| **SIMD buffer diff** | Vectorized cell comparison | Medium |
| Width lookup tables | Cache unicode widths like Ghostty | Medium |
| Style deduplication | RefCountedSet like Ghostty | Medium |

### Tier 3: Architectural Changes

| Enhancement | Description | Effort |
|-------------|-------------|--------|
| Background render thread | Decouple widget building from flush | High |
| Page-based buffer | Ghostty-style memory layout | High |
| Union-based symbol storage | Inline small, offset for graphemes | High |

### Tier 4: Experimental

| Enhancement | Description | Effort |
|-------------|-------------|--------|
| Compute shader diff | GPU-parallel diff like Zutty | Very High |
| wgpu hybrid rendering | Bypass terminal for complex regions | Very High |
| Kitty graphics for previews | Render as images | Medium |

---

## SIMD Diff Implementation

### Why Fork is Required

The diff algorithm lives in `ratatui-core/src/buffer/buffer.rs`:

```rust
pub fn diff<'a>(&self, other: &'a Self) -> Vec<(u16, u16, &'a Cell)> {
    // Sequential cell-by-cell comparison
    for (i, (current, previous)) in ... {
        if current != previous {
            updates.push((x, y, &next_buffer[i]));
        }
    }
}
```

There's no hook to replace this algorithm. Options:

1. **Fork ratatui-core** - Modify `Buffer::diff()` directly ✓
2. **Contribute upstream** - Harder to land, benefits everyone
3. **Custom Terminal wrapper** - Awkward, would reimplement flush

### Patch Strategy

We maintain patches in `lib/patches/` and apply them during build:

```
lib/
├── patches/
│   └── ratatui-core-simd-diff.patch
├── ratatui-core/  (generated - patched source)
└── apply-patches.sh
```

Cargo configuration uses `[patch.crates-io]` to point to local patched version.

### SIMD Diff Algorithm

The key insight is that most of the buffer is unchanged between frames. SIMD can quickly identify changed regions:

```rust
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

pub fn diff_simd<'a>(&self, other: &'a Self) -> Vec<(u16, u16, &'a Cell)> {
    let mut changes = Vec::new();

    // Compare style_ids in chunks of 8 (using 128-bit SIMD)
    // or 16 (using 256-bit AVX2)
    for chunk in cells.chunks(8) {
        // Load style_ids into SIMD registers
        let prev_styles = _mm_loadu_si128(...);
        let curr_styles = _mm_loadu_si128(...);

        // Compare all 8 at once
        let eq = _mm_cmpeq_epi16(prev_styles, curr_styles);
        let mask = _mm_movemask_epi8(eq);

        // If all equal, skip entire chunk
        if mask == 0xFFFF {
            continue;
        }

        // Otherwise, check individual cells in this chunk
        for i in changed_indices(mask) {
            // ... scalar comparison and extraction
        }
    }

    changes
}
```

### Expected Performance Gains

Based on Ghostty's SIMD results and the [ratatui issue #1116](https://github.com/ratatui/ratatui/issues/1116) benchmarks:

| Scenario | Current | With SIMD | Improvement |
|----------|---------|-----------|-------------|
| Mostly static UI | 139ms | ~70ms | 2x |
| Heavy scrolling | Slow | Fast | 4-8x |
| Streaming text | Good (throttled) | Better | 1.5x |

### Additional Optimizations in Patch

1. **Cached unicode widths** - Avoid `unicode_width::width()` calls
2. **Style comparison shortcut** - Compare `style_id` before full Cell
3. **Batch extraction** - Collect changes without allocation per cell

---

## References

### Ghostty

- [Ghostty GitHub](https://github.com/ghostty-org/ghostty)
- [Mitchell Hashimoto - Ghostty Zig Patterns](https://mitchellh.com/writing/ghostty-and-useful-zig-patterns)
- [Ghostty Devlog 005](https://mitchellh.com/writing/ghostty-devlog-005) - Font rendering, shaders
- [Ghostty Devlog 006](https://mitchellh.com/writing/ghostty-devlog-006) - SIMD optimizations
- [DeepWiki - Ghostty Terminal Emulation](https://deepwiki.com/ghostty-org/ghostty/3-terminal-emulation)
- [DeepWiki - Terminal State Management](https://deepwiki.com/ghostty-org/ghostty/3.1-terminal-state-management)

### ratatui

- [ratatui GitHub](https://github.com/ratatui/ratatui)
- [Rendering Under the Hood](https://ratatui.rs/concepts/rendering/under-the-hood/)
- [Issue #1116 - Bypassing Diff](https://github.com/ratatui/ratatui/issues/1116)
- [PR #1605 - Cell Diff Options](https://github.com/ratatui/ratatui/pull/1605)
- [Cell docs](https://docs.rs/ratatui/latest/ratatui/buffer/struct.Cell.html)

### Crossterm

- [Crossterm GitHub](https://github.com/crossterm-rs/crossterm)
- [Performance Enhancement #171](https://github.com/crossterm-rs/crossterm/issues/171)
- [Synchronized Updates](https://docs.rs/crossterm/latest/crossterm/terminal/struct.BeginSynchronizedUpdate.html)

### Other Terminals

- [Zutty - Compute Shader Terminal](https://tomscii.sig7.se/2020/11/How-Zutty-works)
- [Rio Terminal - WebGPU](https://medium.com/@raphamorim/rio-terminal-a-native-and-web-terminal-application-powered-by-rust-webgpu-and-webassembly-76d03a8c99ed)
- [Helix Editor - Synchronized Updates PR](https://github.com/helix-editor/helix/pull/13223)

### Specifications

- [Terminal Synchronized Output Spec](https://gist.github.com/christianparpart/d8a62cc1ab659194337d73e399004036)
- [Kitty Graphics Protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)

---

## Appendix: Ghostty Cell Structure

For reference, Ghostty's cell is much more compact than ratatui's:

```zig
// Ghostty Cell (~16 bytes)
Cell {
    content: union {
        codepoint,        // Single Unicode codepoint
        palette_color,    // Indexed color
        rgb_color,        // True color
        grapheme_offset,  // Offset into grapheme storage
    },
    style_id: u16,        // Reference into RefCountedSet
    wide: enum { narrow, wide, spacer_tail, spacer_head },
    flags: packed { hyperlink, protected },
}

// ratatui Cell (~56 bytes)
Cell {
    symbol: CompactString,  // 24 bytes
    fg: Color,              // 4 bytes
    bg: Color,              // 4 bytes
    underline_color: Color, // 4 bytes
    modifier: Modifier,     // 2 bytes
    skip: bool,             // 1 byte
    // + padding
}
```

This difference explains why Ghostty can process cells much faster - better cache locality and less memory bandwidth.
