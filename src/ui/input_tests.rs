//! Tests for the InputBox widget
//!
//! This module contains render tests that verify the actual terminal output
//! using ratatui's TestBackend, as well as snapshot tests for visual regression.

use super::*;
use ratatui::{backend::TestBackend, Terminal};

/// Helper to render InputBox to a TestBackend buffer and return the buffer content
fn render_input_box(input: &InputBox, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| {
        let widget = input.widget("test-model", 1000);
        frame.render_widget(widget, frame.area());
    }).unwrap();

    // Convert buffer to string representation
    let buffer = terminal.backend().buffer();
    let mut result = String::new();
    for y in 0..height {
        for x in 0..width {
            let cell = buffer.cell((x, y)).unwrap();
            result.push_str(cell.symbol());
        }
        result.push('\n');
    }
    result
}

/// Helper to get just the content area (inside the border)
fn render_input_content(input: &InputBox, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| {
        let widget = input.widget("test-model", 1000);
        frame.render_widget(widget, frame.area());
    }).unwrap();

    // Extract just the inner content (skip border)
    let buffer = terminal.backend().buffer();
    let mut result = String::new();
    for y in 1..(height - 1) {
        for x in 1..(width - 1) {
            let cell = buffer.cell((x, y)).unwrap();
            result.push_str(cell.symbol());
        }
        result.push('\n');
    }
    result
}

// ==================== Render Tests ====================

#[test]
fn test_render_empty_input_shows_placeholder() {
    let input = InputBox::new();
    let rendered = render_input_content(&input, 40, 5);

    assert!(rendered.contains("Type your message here..."),
        "Empty input should show placeholder text");
}

#[test]
fn test_render_typed_text_appears() {
    let mut input = InputBox::new();
    input.insert_char('H');
    input.insert_char('e');
    input.insert_char('l');
    input.insert_char('l');
    input.insert_char('o');

    let rendered = render_input_content(&input, 40, 5);

    assert!(rendered.contains("Hello"),
        "Typed text 'Hello' should appear in rendered output. Got:\n{}", rendered);
}

#[test]
fn test_render_after_backspace() {
    let mut input = InputBox::new();

    // Type "Hello"
    for c in "Hello".chars() {
        input.insert_char(c);
    }
    assert_eq!(input.content(), "Hello");

    // Backspace twice to get "Hel"
    input.delete_char();
    input.delete_char();
    assert_eq!(input.content(), "Hel");

    let rendered = render_input_content(&input, 40, 5);

    assert!(rendered.contains("Hel"),
        "After backspace, 'Hel' should appear. Got:\n{}", rendered);
    assert!(!rendered.contains("Hello"),
        "After backspace, 'Hello' should NOT appear. Got:\n{}", rendered);
}

#[test]
fn test_render_special_characters() {
    let mut input = InputBox::new();

    // Test various special characters
    for c in "!@#$%^&*()".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_content(&input, 40, 5);

    assert!(rendered.contains("!@#$%^&*()"),
        "Special characters should render correctly. Got:\n{}", rendered);
}

#[test]
fn test_render_unicode_characters() {
    let mut input = InputBox::new();

    // Test unicode: emoji, CJK, accented chars
    for c in "Hello".chars() {
        input.insert_char(c);
    }
    input.insert_char(' ');
    for c in "cafe".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_content(&input, 40, 5);

    assert!(rendered.contains("Hello"),
        "Unicode text should render. Got:\n{}", rendered);
}

#[test]
fn test_render_cursor_position_at_end() {
    let mut input = InputBox::new();
    for c in "Test".chars() {
        input.insert_char(c);
    }

    // Cursor should be at position 4 (after "Test")
    assert_eq!(input.cursor(), (0, 4));

    let backend = TestBackend::new(40, 5);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| {
        let widget = input.widget("model", 1000);
        frame.render_widget(widget, frame.area());
    }).unwrap();

    // The cursor cell should have inverted colors (bg=White, fg=Black)
    let buffer = terminal.backend().buffer();
    // Content starts at x=1, y=1 (inside border)
    // Cursor should be at x=1+4=5, y=1
    let cursor_cell = buffer.cell((5, 1)).unwrap();

    assert_eq!(cursor_cell.bg, ratatui::style::Color::White,
        "Cursor position should have White background");
}

#[test]
fn test_render_cursor_position_middle() {
    let mut input = InputBox::new();
    for c in "Hello".chars() {
        input.insert_char(c);
    }

    // Move cursor to middle (after "He")
    input.move_cursor_left(); // after "Hell"
    input.move_cursor_left(); // after "Hel"
    input.move_cursor_left(); // after "He"

    assert_eq!(input.cursor(), (0, 2));

    let backend = TestBackend::new(40, 5);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| {
        let widget = input.widget("model", 1000);
        frame.render_widget(widget, frame.area());
    }).unwrap();

    let buffer = terminal.backend().buffer();
    // Cursor should be at x=1+2=3, y=1 (on the 'l')
    let cursor_cell = buffer.cell((3, 1)).unwrap();

    assert_eq!(cursor_cell.bg, ratatui::style::Color::White,
        "Cursor at middle should have White background");
    assert_eq!(cursor_cell.symbol(), "l",
        "Cursor should be on 'l' character");
}

#[test]
fn test_render_backspace_at_different_positions() {
    let mut input = InputBox::new();
    for c in "ABCDE".chars() {
        input.insert_char(c);
    }

    // Delete from end: "ABCDE" -> "ABCD"
    input.delete_char();
    assert_eq!(input.content(), "ABCD");

    // Move to middle and delete: "ABCD" with cursor after B, delete B -> "ACD"
    input.move_cursor_start();
    input.move_cursor_right(); // after A
    input.move_cursor_right(); // after B
    input.delete_char();       // delete B
    assert_eq!(input.content(), "ACD");

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("ACD"),
        "Content should be 'ACD' after middle deletion. Got:\n{}", rendered);
}

#[test]
fn test_render_newline_wrapping() {
    let mut input = InputBox::new();
    for c in "Line1".chars() {
        input.insert_char(c);
    }
    input.insert_newline();
    for c in "Line2".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_content(&input, 40, 6);

    // Both lines should be present
    assert!(rendered.contains("Line1"),
        "First line should appear. Got:\n{}", rendered);
    assert!(rendered.contains("Line2"),
        "Second line should appear. Got:\n{}", rendered);
}

#[test]
fn test_render_long_text_wraps() {
    let mut input = InputBox::new();
    let long_text = "This is a very long line that should wrap around";
    for c in long_text.chars() {
        input.insert_char(c);
    }

    // Render in a narrow box (20 chars wide, minus 2 for borders = 18 inner)
    let rendered = render_input_content(&input, 20, 6);

    // The text should be split across multiple lines
    let lines: Vec<&str> = rendered.lines().collect();
    assert!(lines.len() >= 2,
        "Long text should wrap to multiple lines. Got {} lines:\n{}", lines.len(), rendered);
}

#[test]
fn test_render_border_and_title() {
    let input = InputBox::new();
    let rendered = render_input_box(&input, 40, 5);

    // Should contain the model name in title
    assert!(rendered.contains("test-model"),
        "Border should show model name. Got:\n{}", rendered);
}

#[test]
fn test_render_token_count_display() {
    let input = InputBox::new();

    // Render with 5000 tokens (should show "5k")
    let backend = TestBackend::new(40, 5);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|frame| {
        let widget = input.widget("model", 5000);
        frame.render_widget(widget, frame.area());
    }).unwrap();

    let buffer = terminal.backend().buffer();
    let mut full_render = String::new();
    for y in 0..5 {
        for x in 0..40 {
            full_render.push_str(buffer.cell((x, y)).unwrap().symbol());
        }
        full_render.push('\n');
    }

    assert!(full_render.contains("5k"),
        "Should display '5k' for 5000 tokens. Got:\n{}", full_render);
}

// ==================== Snapshot Tests ====================

#[test]
fn test_snapshot_empty_input() {
    let input = InputBox::new();
    let rendered = render_input_box(&input, 50, 5);
    insta::assert_snapshot!(rendered);
}

#[test]
fn test_snapshot_with_text() {
    let mut input = InputBox::new();
    for c in "Hello, world!".chars() {
        input.insert_char(c);
    }
    let rendered = render_input_box(&input, 50, 5);
    insta::assert_snapshot!(rendered);
}

#[test]
fn test_snapshot_multiline() {
    let mut input = InputBox::new();
    for c in "First line".chars() {
        input.insert_char(c);
    }
    input.insert_newline();
    for c in "Second line".chars() {
        input.insert_char(c);
    }
    let rendered = render_input_box(&input, 50, 6);
    insta::assert_snapshot!(rendered);
}

#[test]
fn test_snapshot_special_chars() {
    let mut input = InputBox::new();
    for c in "Special: !@#$%^&*() <>[]{} '\"`~".chars() {
        input.insert_char(c);
    }
    let rendered = render_input_box(&input, 50, 5);
    insta::assert_snapshot!(rendered);
}

#[test]
fn test_snapshot_wrapped_long_text() {
    let mut input = InputBox::new();
    let text = "This is a much longer piece of text that will definitely need to wrap across multiple lines when rendered in a narrow terminal window";
    for c in text.chars() {
        input.insert_char(c);
    }
    let rendered = render_input_box(&input, 40, 8);
    insta::assert_snapshot!(rendered);
}

// ==================== Navigation + Edit Tests ====================

#[test]
fn test_insert_in_middle_of_text() {
    let mut input = InputBox::new();

    // Type "AC"
    input.insert_char('A');
    input.insert_char('C');
    assert_eq!(input.content(), "AC");

    // Move left and insert B -> "ABC"
    input.move_cursor_left();
    input.insert_char('B');

    assert_eq!(input.content(), "ABC");
    assert_eq!(input.cursor(), (0, 2)); // cursor after B

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("ABC"),
        "Should render 'ABC' after mid-insert. Got:\n{}", rendered);
}

#[test]
fn test_multiple_insertions_at_different_positions() {
    let mut input = InputBox::new();

    // Type "15"
    input.insert_char('1');
    input.insert_char('5');

    // Go to start, insert "0" -> "015"
    input.move_cursor_start();
    input.insert_char('0');
    assert_eq!(input.content(), "015");

    // Go to end, insert "6" -> "0156"
    input.move_cursor_end();
    input.insert_char('6');
    assert_eq!(input.content(), "0156");

    // Navigate to middle (after "01"), insert "234" -> "0123456"
    input.move_cursor_start();
    input.move_cursor_right(); // after 0
    input.move_cursor_right(); // after 1
    input.insert_char('2');
    input.insert_char('3');
    input.insert_char('4');
    assert_eq!(input.content(), "0123456");

    // Move to end and verify
    input.move_cursor_end();
    assert_eq!(input.cursor(), (0, 7));

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("0123456"),
        "Should render '0123456'. Got:\n{}", rendered);
}

#[test]
fn test_delete_after_navigation() {
    let mut input = InputBox::new();

    // Type "ABCDE"
    for c in "ABCDE".chars() {
        input.insert_char(c);
    }

    // Navigate to after C, delete C -> "ABDE"
    input.move_cursor_left(); // after D
    input.move_cursor_left(); // after C
    input.delete_char();
    assert_eq!(input.content(), "ABDE");

    // Delete B -> "ADE"
    input.delete_char();
    assert_eq!(input.content(), "ADE");

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("ADE"),
        "Should render 'ADE'. Got:\n{}", rendered);
}

#[test]
fn test_interleaved_navigation_insert_delete() {
    let mut input = InputBox::new();

    // Build "Hello" via mixed operations
    input.insert_char('H');
    input.insert_char('l');     // "Hl"
    input.move_cursor_left();
    input.insert_char('e');     // "Hel"
    input.move_cursor_end();
    input.insert_char('l');     // "Hell"
    input.insert_char('o');     // "Hello"

    assert_eq!(input.content(), "Hello");

    // Now transform to "Help" via navigation and edits
    input.move_cursor_left();   // before 'o'
    input.delete_char();        // delete 'l' -> "Helo"
    input.move_cursor_left();   // before 'o'
    input.delete_char();        // delete 'l' -> "Heo"

    // Oops, that's wrong. Let's fix it differently.
    // Start fresh
    input.clear();
    for c in "Hello".chars() {
        input.insert_char(c);
    }

    // Transform "Hello" -> "Help!"
    input.delete_char();        // "Hell"
    input.delete_char();        // "Hel"
    input.insert_char('p');     // "Help"
    input.insert_char('!');     // "Help!"

    assert_eq!(input.content(), "Help!");

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("Help!"),
        "Should render 'Help!'. Got:\n{}", rendered);
}

#[test]
fn test_cursor_boundaries() {
    let mut input = InputBox::new();

    // Empty input - cursor should stay at 0
    input.move_cursor_left();
    assert_eq!(input.cursor(), (0, 0));
    input.move_cursor_left();
    assert_eq!(input.cursor(), (0, 0));

    // Type "AB"
    input.insert_char('A');
    input.insert_char('B');

    // At end, move right should stay at end
    input.move_cursor_right();
    assert_eq!(input.cursor(), (0, 2));
    input.move_cursor_right();
    assert_eq!(input.cursor(), (0, 2));

    // At start, move left should stay at start
    input.move_cursor_start();
    input.move_cursor_left();
    assert_eq!(input.cursor(), (0, 0));
}

#[test]
fn test_delete_at_start_does_nothing() {
    let mut input = InputBox::new();

    input.insert_char('X');
    input.move_cursor_start();
    input.delete_char(); // Should do nothing - nothing before cursor

    assert_eq!(input.content(), "X");
    assert_eq!(input.cursor(), (0, 0));
}

#[test]
fn test_rapid_insert_delete_cycle() {
    let mut input = InputBox::new();

    // Rapid typing and deleting
    for _ in 0..5 {
        input.insert_char('a');
        input.insert_char('b');
        input.delete_char();
    }
    // Should have "aaaaa"
    assert_eq!(input.content(), "aaaaa");

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("aaaaa"),
        "Should render 'aaaaa'. Got:\n{}", rendered);
}

#[test]
fn test_navigate_and_overwrite_pattern() {
    let mut input = InputBox::new();

    // Type "XXXXX"
    for _ in 0..5 {
        input.insert_char('X');
    }

    // Replace each X with a digit by navigating and delete+insert
    input.move_cursor_start();
    for i in 1..=5 {
        input.move_cursor_right(); // move past current char
        input.delete_char();       // delete the char we just passed
        input.insert_char(char::from_digit(i, 10).unwrap());
    }

    assert_eq!(input.content(), "12345");

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("12345"),
        "Should render '12345'. Got:\n{}", rendered);
}

#[test]
fn test_cursor_position_after_complex_edits() {
    let mut input = InputBox::new();

    // Type "abcdef"
    for c in "abcdef".chars() {
        input.insert_char(c);
    }

    // Navigate to middle (after 'c')
    input.move_cursor_start();
    input.move_cursor_right(); // after a
    input.move_cursor_right(); // after b
    input.move_cursor_right(); // after c

    // Insert "123" -> "abc123def"
    input.insert_char('1');
    input.insert_char('2');
    input.insert_char('3');

    assert_eq!(input.content(), "abc123def");
    assert_eq!(input.cursor(), (0, 6)); // after "abc123"

    // Verify cursor renders at correct position
    let backend = TestBackend::new(40, 5);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| {
        f.render_widget(input.widget("m", 0), f.area());
    }).unwrap();

    let buffer = terminal.backend().buffer();
    // Cursor at x=1+6=7, y=1
    let cursor_cell = buffer.cell((7, 1)).unwrap();
    assert_eq!(cursor_cell.bg, ratatui::style::Color::White,
        "Cursor should be at position 6 (after '3')");
    assert_eq!(cursor_cell.symbol(), "d",
        "Cursor should be on 'd'");
}

#[test]
fn test_delete_entire_content_and_retype() {
    let mut input = InputBox::new();

    // Type "Hello"
    for c in "Hello".chars() {
        input.insert_char(c);
    }

    // Delete everything
    for _ in 0..5 {
        input.delete_char();
    }
    assert_eq!(input.content(), "");
    assert!(input.is_empty());

    // Retype "World"
    for c in "World".chars() {
        input.insert_char(c);
    }
    assert_eq!(input.content(), "World");

    let rendered = render_input_content(&input, 40, 5);
    assert!(rendered.contains("World"),
        "Should render 'World' after delete-all and retype. Got:\n{}", rendered);
    assert!(!rendered.contains("Hello"),
        "Should NOT contain 'Hello'. Got:\n{}", rendered);
}

#[test]
fn test_snapshot_after_complex_edits() {
    let mut input = InputBox::new();

    // Complex edit sequence
    for c in "The quick brown".chars() {
        input.insert_char(c);
    }
    // Insert " fox" at end
    for c in " fox".chars() {
        input.insert_char(c);
    }
    // Go back and fix "brown" to "red"
    // Current: "The quick brown fox"
    // Navigate to after "quick "
    input.move_cursor_start();
    for _ in 0..10 { // "The quick "
        input.move_cursor_right();
    }
    // Delete "brown" (5 chars)
    for _ in 0..5 {
        input.move_cursor_right();
    }
    for _ in 0..5 {
        input.delete_char();
    }
    // Insert "red"
    for c in "red".chars() {
        input.insert_char(c);
    }

    assert_eq!(input.content(), "The quick red fox");

    let rendered = render_input_box(&input, 40, 5);
    insta::assert_snapshot!(rendered);
}

// ==================== Original Logic Tests ====================

#[test]
fn test_input_box_basic() {
    let mut input = InputBox::new();

    input.insert_char('H');
    input.insert_char('i');
    assert_eq!(input.content(), "Hi");
    assert_eq!(input.cursor(), (0, 2));

    input.delete_char();
    assert_eq!(input.content(), "H");
    assert_eq!(input.cursor(), (0, 1));
}

#[test]
fn test_input_box_cursor_movement() {
    let mut input = InputBox::new();
    input.insert_char('a');
    input.insert_char('b');
    input.insert_char('c');

    input.move_cursor_left();
    assert_eq!(input.cursor(), (0, 2));

    input.move_cursor_start();
    assert_eq!(input.cursor(), (0, 0));

    input.move_cursor_end();
    assert_eq!(input.cursor(), (0, 3));
}

#[test]
fn test_input_box_history() {
    let mut input = InputBox::new();

    input.insert_char('a');
    input.submit();

    input.insert_char('b');
    input.submit();

    input.history_prev();
    assert_eq!(input.content(), "b");

    input.history_prev();
    assert_eq!(input.content(), "a");

    input.history_next();
    assert_eq!(input.content(), "b");
}

#[test]
fn test_attachment() {
    let mut input = InputBox::new();
    input.insert_char('a');
    input.add_attachment(Attachment::pasted("file contents".to_string()));
    input.insert_char('b');

    assert_eq!(input.content(), "ab");  // Text only
    assert_eq!(input.segments().len(), 3);  // Text, Attachment, Text

    let expanded = input.submit();
    assert_eq!(expanded, "afile contentsb");  // Expanded with content
}

// ==================== Attachment Render Tests ====================

#[test]
fn test_render_pasted_text_pill() {
    let mut input = InputBox::new();
    input.add_attachment(Attachment::pasted("some pasted content".to_string()));

    let rendered = render_input_content(&input, 50, 5);

    // Should show the pilcrow icon and char count
    assert!(rendered.contains("¶"),
        "Pasted text should show pilcrow (¶) icon. Got:\n{}", rendered);
    assert!(rendered.contains("pasted"),
        "Should show 'pasted' label. Got:\n{}", rendered);
    assert!(rendered.contains("19 chars"),
        "Should show char count. Got:\n{}", rendered);
}

#[test]
fn test_render_ide_selection_pill() {
    let mut input = InputBox::new();
    input.add_attachment(Attachment::ide_selection(
        "/path/to/file.rs".to_string(),
        "fn main() {}".to_string(),
        10,
        15,
    ));

    let rendered = render_input_content(&input, 50, 5);

    // Should show section icon, filename, and line range
    assert!(rendered.contains("§"),
        "IDE selection should show section (§) icon. Got:\n{}", rendered);
    assert!(rendered.contains("file.rs"),
        "Should show filename. Got:\n{}", rendered);
    assert!(rendered.contains("10-15"),
        "Should show line range. Got:\n{}", rendered);
}

#[test]
fn test_render_ide_selection_single_line() {
    let mut input = InputBox::new();
    input.add_attachment(Attachment::ide_selection(
        "/src/lib.rs".to_string(),
        "let x = 42;".to_string(),
        42,
        42,  // Same line
    ));

    let rendered = render_input_content(&input, 50, 5);

    // Single line should show just the line number, not a range
    assert!(rendered.contains("lib.rs:42"),
        "Single line should show 'filename:line'. Got:\n{}", rendered);
    assert!(!rendered.contains("42-42"),
        "Should NOT show range for single line. Got:\n{}", rendered);
}

#[test]
fn test_render_text_with_pasted_attachment() {
    let mut input = InputBox::new();

    // Type some text, paste, then type more
    for c in "Before ".chars() {
        input.insert_char(c);
    }
    input.add_attachment(Attachment::pasted("PASTED".to_string()));
    for c in " After".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_content(&input, 60, 5);

    assert!(rendered.contains("Before"),
        "Should contain text before attachment. Got:\n{}", rendered);
    assert!(rendered.contains("After"),
        "Should contain text after attachment. Got:\n{}", rendered);
    assert!(rendered.contains("¶"),
        "Should contain paste indicator. Got:\n{}", rendered);
}

#[test]
fn test_render_multiple_attachments() {
    let mut input = InputBox::new();

    input.add_attachment(Attachment::pasted("first paste".to_string()));
    for c in " middle ".chars() {
        input.insert_char(c);
    }
    input.add_attachment(Attachment::pasted("second paste".to_string()));

    let rendered = render_input_content(&input, 80, 5);

    // Count pilcrow occurrences
    let pilcrow_count = rendered.matches("¶").count();
    assert_eq!(pilcrow_count, 2,
        "Should have 2 paste indicators. Got {} in:\n{}", pilcrow_count, rendered);
    assert!(rendered.contains("middle"),
        "Should contain middle text. Got:\n{}", rendered);
}

#[test]
fn test_render_attachment_with_navigation() {
    let mut input = InputBox::new();

    // Add attachment then type after it
    input.add_attachment(Attachment::pasted("content".to_string()));
    for c in "typed".chars() {
        input.insert_char(c);
    }

    // Navigate before attachment and add text there
    input.move_cursor_start();
    for c in "prefix ".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_content(&input, 60, 5);

    assert!(rendered.contains("prefix"),
        "Should contain prefix text. Got:\n{}", rendered);
    assert!(rendered.contains("typed"),
        "Should contain typed text after attachment. Got:\n{}", rendered);
}

#[test]
fn test_render_delete_attachment() {
    let mut input = InputBox::new();

    for c in "before".chars() {
        input.insert_char(c);
    }
    input.add_attachment(Attachment::pasted("will be deleted".to_string()));
    for c in "after".chars() {
        input.insert_char(c);
    }

    // Verify attachment is there
    assert_eq!(input.segments().len(), 3);

    // Navigate to just after the attachment and delete it
    input.move_cursor_start();
    for _ in 0..6 { // "before"
        input.move_cursor_right();
    }
    input.move_cursor_right(); // Move onto/past attachment
    input.delete_char();       // Delete the attachment

    let rendered = render_input_content(&input, 60, 5);

    assert!(!rendered.contains("¶"),
        "Attachment should be deleted. Got:\n{}", rendered);
    assert!(rendered.contains("before"),
        "Should still have 'before'. Got:\n{}", rendered);
    assert!(rendered.contains("after"),
        "Should still have 'after'. Got:\n{}", rendered);
}

#[test]
fn test_snapshot_with_pasted_attachment() {
    let mut input = InputBox::new();

    for c in "Check this: ".chars() {
        input.insert_char(c);
    }
    input.add_attachment(Attachment::pasted("code snippet here".to_string()));

    let rendered = render_input_box(&input, 60, 5);
    insta::assert_snapshot!(rendered);
}

#[test]
fn test_snapshot_with_ide_selection() {
    let mut input = InputBox::new();

    input.add_attachment(Attachment::ide_selection(
        "/src/main.rs".to_string(),
        "fn main() {\n    println!(\"Hello\");\n}".to_string(),
        1,
        3,
    ));
    for c in " fix this function".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_box(&input, 60, 5);
    insta::assert_snapshot!(rendered);
}

#[test]
fn test_set_ide_selection_updates_existing() {
    let mut input = InputBox::new();

    // Set initial selection
    input.set_ide_selection(Some(Attachment::ide_selection(
        "/old.rs".to_string(),
        "old content".to_string(),
        1, 1,
    )));

    let rendered1 = render_input_content(&input, 50, 5);
    assert!(rendered1.contains("old.rs"),
        "Should show old filename. Got:\n{}", rendered1);

    // Update selection
    input.set_ide_selection(Some(Attachment::ide_selection(
        "/new.rs".to_string(),
        "new content".to_string(),
        10, 20,
    )));

    let rendered2 = render_input_content(&input, 50, 5);
    assert!(rendered2.contains("new.rs"),
        "Should show new filename. Got:\n{}", rendered2);
    assert!(!rendered2.contains("old.rs"),
        "Should NOT show old filename. Got:\n{}", rendered2);

    // Only one IDE selection should exist
    let ide_count = input.segments().iter().filter(|s| {
        matches!(s, Segment::Attachment(a) if matches!(a.kind, AttachmentKind::IdeSelection { .. }))
    }).count();
    assert_eq!(ide_count, 1, "Should have exactly 1 IDE selection");
}

#[test]
fn test_clear_ide_selection() {
    let mut input = InputBox::new();

    input.set_ide_selection(Some(Attachment::ide_selection(
        "/file.rs".to_string(),
        "content".to_string(),
        1, 1,
    )));

    let rendered1 = render_input_content(&input, 50, 5);
    assert!(rendered1.contains("§"), "Should have IDE selection");

    // Clear it
    input.set_ide_selection(None);

    let rendered2 = render_input_content(&input, 50, 5);
    assert!(!rendered2.contains("§"),
        "IDE selection should be cleared. Got:\n{}", rendered2);
}
