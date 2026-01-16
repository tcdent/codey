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

/// Create expected content string with proper padding
/// Content area width is widget_width - 2 (for borders)
fn expected_content(lines: &[&str], width: u16, height: u16) -> String {
    let content_width = (width - 2) as usize;
    let content_height = (height - 2) as usize;
    let mut result = String::new();

    for i in 0..content_height {
        let line = lines.get(i).copied().unwrap_or("");
        result.push_str(line);
        // Pad with spaces to fill the width
        for _ in line.chars().count()..content_width {
            result.push(' ');
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

    assert_eq!(rendered, "Type your message here...\n\n");
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

    assert_eq!(rendered, "Hello\n\n");
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

    assert_eq!(rendered, "Hel\n\n");
}

#[test]
fn test_render_special_characters() {
    let mut input = InputBox::new();

    // Test various special characters
    for c in "!@#$%^&*()".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_content(&input, 40, 5);

    assert_eq!(rendered, "!@#$%^&*()\n\n");
}

#[test]
fn test_render_unicode_characters() {
    let mut input = InputBox::new();

    // Test unicode
    for c in "Hello".chars() {
        input.insert_char(c);
    }
    input.insert_char(' ');
    for c in "cafe".chars() {
        input.insert_char(c);
    }

    let rendered = render_input_content(&input, 40, 5);

    assert_eq!(rendered, "Hello cafe\n\n");
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
    assert_eq!(rendered, "ACD\n\n");
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

    assert_eq!(rendered, "Line1\nLine2\n\n");
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
    assert_eq!(rendered, "ABC\n\n");
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
    assert_eq!(rendered, "0123456\n\n");
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
    assert_eq!(rendered, "ADE\n\n");
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
    assert_eq!(rendered, "Help!\n\n");
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
    assert_eq!(rendered, "aaaaa\n\n");
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
    assert_eq!(rendered, "12345\n\n");
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
    assert_eq!(rendered, "World\n\n");
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

    assert_eq!(rendered, "[¶ pasted (19 chars)]\n\n");
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

    assert_eq!(rendered, "[§ file.rs:10-15]\n\n");
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
    assert_eq!(rendered, "[§ lib.rs:42]\n\n");
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

    assert_eq!(rendered, "Before [¶ pasted (6 chars)]  After\n\n");
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

    assert_eq!(rendered, "[¶ pasted (11 chars)]  middle [¶ pasted (12 chars)]\n\n");
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

    assert_eq!(rendered, "prefix [¶ pasted (7 chars)] typed\n\n");
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

    assert_eq!(rendered, "beforeafter\n\n");
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
    assert_eq!(rendered1, "[§ old.rs:1]\n\n");

    // Update selection
    input.set_ide_selection(Some(Attachment::ide_selection(
        "/new.rs".to_string(),
        "new content".to_string(),
        10, 20,
    )));

    let rendered2 = render_input_content(&input, 50, 5);
    assert_eq!(rendered2, "[§ new.rs:10-20]\n\n");

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
    assert_eq!(rendered1, "[§ file.rs:1]\n\n");

    // Clear it
    input.set_ide_selection(None);

    let rendered2 = render_input_content(&input, 50, 5);
    assert_eq!(rendered2, "Type your message here...\n\n");
}
