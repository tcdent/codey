# Terminal UI Testing Research

This document captures research on testing strategies for interactive terminal applications, specifically for ratatui-based TUIs like Codey.

## Table of Contents

1. [The Challenge](#the-challenge)
2. [Testing Tiers](#testing-tiers)
3. [Tier 1: Unit Tests with TestBackend](#tier-1-unit-tests-with-testbackend)
4. [Tier 2: Snapshot Testing with insta](#tier-2-snapshot-testing-with-insta)
5. [Tier 3: PTY Integration Testing](#tier-3-pty-integration-testing)
6. [Tier 4: Expect-Style Automation](#tier-4-expect-style-automation)
7. [Implementation in Codey](#implementation-in-codey)
8. [References](#references)

---

## The Challenge

Testing interactive terminal UIs is challenging because:

1. **Rendering is stateful** - The terminal maintains cursor position, styles, and a 2D character grid
2. **Input is event-based** - Keypresses, paste events, and resize events must be simulated
3. **Output includes escape codes** - Raw terminal output is ANSI escape sequences, not readable text
4. **Async behavior** - Many TUIs have streaming responses, background tasks, and concurrent event handling

Traditional unit testing only validates internal logic, not the actual rendered output that users see.

---

## Testing Tiers

We recommend a multi-tier testing strategy:

```
┌─────────────────────────────────────────────────────────────┐
│  Tier 4: Expect-Style Automation (expectrl)                 │
│  - Full process spawning                                    │
│  - Pattern matching on output                               │
│  - CI/CD integration testing                                │
├─────────────────────────────────────────────────────────────┤
│  Tier 3: PTY Integration (ratatui-testlib)                  │
│  - Real pseudo-terminal                                     │
│  - Keyboard event injection                                 │
│  - Full application lifecycle                               │
├─────────────────────────────────────────────────────────────┤
│  Tier 2: Snapshot Testing (insta)                           │
│  - Visual regression detection                              │
│  - Human-reviewable diffs                                   │
│  - Widget render verification                               │
├─────────────────────────────────────────────────────────────┤
│  Tier 1: Unit Tests (TestBackend)                           │
│  - Buffer content assertions                                │
│  - Cursor position verification                             │
│  - Style/color checking                                     │
│  - Fast, isolated, deterministic                            │
└─────────────────────────────────────────────────────────────┘
```

---

## Tier 1: Unit Tests with TestBackend

Ratatui provides `TestBackend` which captures rendered output to an in-memory buffer instead of writing to a real terminal.

### What It Tests

- **Actual rendered characters** in the 2D grid
- **Cursor position** after rendering
- **Styles** (foreground/background colors, bold, italic, etc.)
- **Layout calculations** (wrapping, truncation, positioning)

### Example

```rust
use ratatui::{backend::TestBackend, Terminal};

#[test]
fn test_input_renders_text() {
    let backend = TestBackend::new(40, 5);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut input = InputBox::new();
    input.insert_char('H');
    input.insert_char('i');

    terminal.draw(|frame| {
        frame.render_widget(input.widget("model", 0), frame.area());
    }).unwrap();

    let buffer = terminal.backend().buffer();

    // Verify specific cell content
    assert_eq!(buffer.cell((1, 1)).unwrap().symbol(), "H");
    assert_eq!(buffer.cell((2, 1)).unwrap().symbol(), "i");

    // Verify cursor position (inverted colors)
    let cursor_cell = buffer.cell((3, 1)).unwrap();
    assert_eq!(cursor_cell.bg, Color::White);
}
```

### Buffer Assertion Helpers

```rust
/// Render to string for easy assertion
fn render_to_string(input: &InputBox, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| {
        frame.render_widget(input.widget("model", 0), frame.area());
    }).unwrap();

    let buffer = terminal.backend().buffer();
    let mut result = String::new();
    for y in 0..height {
        for x in 0..width {
            result.push_str(buffer.cell((x, y)).unwrap().symbol());
        }
        result.push('\n');
    }
    result
}
```

### Key Point: Buffer = Rendered Reality

The `TestBackend` buffer is NOT just the internal state variable. It's the **actual post-render output** after:
- Layout calculations
- Word wrapping
- Truncation/overflow handling
- Style application
- Cursor positioning

This is why it's valuable for UI testing - you're testing what users actually see.

---

## Tier 2: Snapshot Testing with insta

[insta](https://crates.io/crates/insta) provides snapshot testing - capturing rendered output and comparing against known-good baselines.

### Setup

```toml
# Cargo.toml
[dev-dependencies]
insta = "1.40"
```

### Example

```rust
#[test]
fn test_snapshot_input_with_text() {
    let mut input = InputBox::new();
    for c in "Hello, world!".chars() {
        input.insert_char(c);
    }
    let rendered = render_to_string(&input, 50, 5);
    insta::assert_snapshot!(rendered);
}
```

### Workflow

1. **First run**: insta creates `.snap.new` files in `snapshots/` directory
2. **Review**: Run `cargo insta review` to accept/reject changes
3. **Commit**: Accepted snapshots become the baseline
4. **CI**: Tests fail if output differs from committed snapshots

### Benefits

- **Visual diffs** - Easy to see what changed in the UI
- **Regression detection** - Catches unintended UI changes
- **Documentation** - Snapshots serve as visual documentation of expected output

---

## Tier 3: PTY Integration Testing

[ratatui-testlib](https://lib.rs/crates/ratatui-testlib) runs applications in a real pseudo-terminal (PTY), enabling full integration testing.

### Setup

```toml
[dev-dependencies]
ratatui-testlib = { version = "0.1", features = ["mvp"] }
```

### Example

```rust
use ratatui_testlib::{TuiTestHarness, Result};
use portable_pty::CommandBuilder;

#[test]
fn test_full_interaction() -> Result<()> {
    let mut harness = TuiTestHarness::new(80, 24)?;
    harness.spawn(CommandBuilder::new("./target/debug/codey"))?;

    // Wait for app to initialize
    harness.wait_for(|state| state.contents().contains(">"))?;

    // Type input
    harness.send_text("Hello, Claude!")?;

    // Verify it appears in rendered output
    harness.wait_for(|state| {
        state.contents().contains("Hello, Claude!")
    })?;

    // Snapshot the final state
    insta::assert_snapshot!(harness.screen_contents());

    Ok(())
}
```

### What It Tests

- **Full application lifecycle** - Startup, interaction, shutdown
- **Keyboard event handling** - Real keypress simulation
- **Terminal negotiation** - Size, capabilities, etc.
- **Async behavior** - Streaming responses, concurrent events

### Limitations

- Slower than unit tests
- Requires building the binary
- More complex setup for CI

---

## Tier 4: Expect-Style Automation

[expectrl](https://github.com/zhiburt/expectrl) provides classic Unix `expect`-style automation for terminal applications.

### Setup

```toml
[dev-dependencies]
expectrl = "0.8"
```

### Example

```rust
use expectrl::{spawn, Regex};

#[test]
fn test_cli_interaction() {
    let mut session = spawn("./target/debug/codey").unwrap();

    // Wait for prompt
    session.expect(Regex(">")).unwrap();

    // Send input
    session.send_line("test message").unwrap();

    // Expect response
    session.expect("test message").unwrap();

    // Send quit command
    session.send("\x03").unwrap();  // Ctrl+C
}
```

### Use Cases

- CI/CD integration testing
- End-to-end workflow testing
- Testing CLI argument handling
- Regression testing for specific user flows

---

## Implementation in Codey

### Current Implementation

Tests are located in `src/ui/input.rs` and include:

**Render Tests:**
- `test_render_empty_input_shows_placeholder` - Verifies placeholder text
- `test_render_typed_text_appears` - Verifies typed text is rendered
- `test_render_after_backspace` - Verifies backspace affects rendered output
- `test_render_special_characters` - Verifies special chars render correctly
- `test_render_unicode_characters` - Verifies unicode handling
- `test_render_cursor_position_at_end` - Verifies cursor styling at end
- `test_render_cursor_position_middle` - Verifies cursor styling in middle
- `test_render_backspace_at_different_positions` - Tests mid-text deletion
- `test_render_newline_wrapping` - Tests multiline rendering
- `test_render_long_text_wraps` - Tests word wrap
- `test_render_border_and_title` - Verifies border/title rendering
- `test_render_token_count_display` - Verifies token count in title

**Snapshot Tests:**
- `test_snapshot_empty_input`
- `test_snapshot_with_text`
- `test_snapshot_multiline`
- `test_snapshot_special_chars`
- `test_snapshot_wrapped_long_text`

### Helper Functions

```rust
/// Render full input box including border
fn render_input_box(input: &InputBox, width: u16, height: u16) -> String

/// Render just the content area (inside border)
fn render_input_content(input: &InputBox, width: u16, height: u16) -> String
```

### Running Tests

```bash
# Run all input tests
cargo test ui::input::tests

# Run with snapshot review
cargo insta test
cargo insta review

# Run specific test
cargo test test_render_after_backspace
```

---

## Best Practices

### 1. Use Consistent Terminal Sizes

Always use fixed dimensions (e.g., 80x24) for reproducible tests:

```rust
let backend = TestBackend::new(80, 24);  // Standard terminal size
```

### 2. Test Edge Cases

- Empty input
- Single character
- Max length input
- Unicode characters (emoji, CJK, RTL)
- Special characters (tab, newline, escape)
- Cursor at start/middle/end

### 3. Separate Logic Tests from Render Tests

```rust
// Logic test - fast, no rendering
#[test]
fn test_backspace_removes_char() {
    let mut input = InputBox::new();
    input.insert_char('a');
    input.delete_char();
    assert_eq!(input.content(), "");
}

// Render test - verifies visual output
#[test]
fn test_render_after_backspace() {
    let mut input = InputBox::new();
    input.insert_char('a');
    input.delete_char();
    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("Type your message"));  // Shows placeholder
}
```

### 4. Use Snapshots for Complex Layouts

For complex multi-widget layouts, snapshot testing is more maintainable than per-cell assertions:

```rust
#[test]
fn test_full_chat_layout() {
    // ... setup chat view with messages ...
    let rendered = render_full_ui();
    insta::assert_snapshot!(rendered);
}
```

---

## References

- [ratatui TestBackend docs](https://docs.rs/ratatui/latest/ratatui/backend/struct.TestBackend.html)
- [ratatui Testing with insta snapshots](https://ratatui.rs/recipes/testing/snapshots/)
- [ratatui-testlib](https://lib.rs/crates/ratatui-testlib)
- [expectrl](https://github.com/zhiburt/expectrl)
- [insta snapshot testing](https://crates.io/crates/insta)
