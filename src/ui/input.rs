//! Input box component with word-wrap and cursor positioning
//!
//! We evaluated several existing crates for text input:
//!
//! - **tui-textarea**: Mature crate but doesn't support word-wrap - it scrolls
//!   horizontally like a code editor. Not suitable for chat input.
//!
//! - **rat-text**: Has word-wrap support, but requires ratatui 0.29 (we use 0.30)
//!   and pulls in many dependencies (rat-cursor, rat-event, rat-focus, etc.).
//!
//! The core challenge is cursor positioning with word-wrap. Ratatui's `Paragraph`
//! widget does word-boundary wrapping, but doesn't expose where text ends up after
//! wrapping. We use `textwrap` crate to pre-wrap text, then:
//!
//! 1. Render the pre-wrapped lines directly (no `Paragraph::wrap()`)
//! 2. Calculate cursor position using the same wrapped output
//!
//! This guarantees cursor and display stay in sync. One quirk: `textwrap` trims
//! trailing spaces, so we count them separately and add to cursor position.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use textwrap::wrap;
use unicode_width::UnicodeWidthStr;

/// Format a token count with "k" suffix for thousands
fn format_tokens(count: u32) -> String {
    if count >= 1000 {
        format!("{}k", count / 1000)
    } else {
        "<1k".to_string()
    }
}

/// Type of attached content
#[derive(Debug, Clone)]
pub enum AttachmentKind {
    /// Pasted text content
    PastedText { char_count: usize },
    /// Selection from the IDE
    IdeSelection {
        path: String,
        start_line: u32,
        end_line: u32,
    },
}

/// Attached content shown as a pill
#[derive(Debug, Clone)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub content: String,
}

impl Attachment {
    /// Create a new pasted text attachment
    pub fn pasted(content: String) -> Self {
        Self {
            kind: AttachmentKind::PastedText { char_count: content.len() },
            content,
        }
    }

    /// Create a new IDE selection attachment
    pub fn ide_selection(path: String, content: String, start_line: u32, end_line: u32) -> Self {
        Self {
            kind: AttachmentKind::IdeSelection { path, start_line, end_line },
            content,
        }
    }

    /// Get the label for this attachment
    pub fn label(&self) -> String {
        match &self.kind {
            AttachmentKind::PastedText { char_count } => format!("pasted ({} chars)", char_count),
            AttachmentKind::IdeSelection { path, start_line, end_line } => {
                // Extract just the filename from the path
                let filename = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path);
                if start_line == end_line {
                    format!("{}:{}", filename, start_line)
                } else {
                    format!("{}:{}-{}", filename, start_line, end_line)
                }
            }
        }
    }

    /// Get the display string (pill format with trailing space)
    pub fn display(&self) -> String {
        let icon = match &self.kind {
            AttachmentKind::PastedText { .. } => "\u{00B6}",  // ¶ pilcrow
            AttachmentKind::IdeSelection { .. } => "\u{00A7}",  // § section
        };
        format!("[{} {}] ", icon, self.label())
    }

    /// Get the expanded content for the prompt
    pub fn expanded(&self) -> String {
        match &self.kind {
            AttachmentKind::PastedText { .. } => {
                self.content.clone()
            }
            AttachmentKind::IdeSelection { path, start_line, end_line } => {
                // Format with line numbers like read_file does
                let line_num_width = end_line.to_string().len().max(4);
                let mut numbered_content = String::new();
                
                for (i, line) in self.content.lines().enumerate() {
                    let line_num = start_line + i as u32;
                    numbered_content.push_str(&format!(
                        "{:>width$}\u{2502}{}\n",
                        line_num,
                        line,
                        width = line_num_width as usize
                    ));
                }
                
                let range = if start_line == end_line {
                    format!("{}", start_line)
                } else {
                    format!("{}-{}", start_line, end_line)
                };
                format!("\n```\n# {}:{}\n{}```\n\n", path, range, numbered_content)
            }
        }
    }
}

/// A segment of input - either typed text or an attachment pill
#[derive(Debug, Clone)]
pub enum Segment {
    Text(String),
    Attachment(Attachment),
}

impl Segment {
    /// Byte length for cursor positioning within this segment.
    /// Text: full string length, Attachment: 0 (cursor sits at position 0)
    fn len(&self) -> usize {
        match self {
            Segment::Text(s) => s.len(),
            Segment::Attachment(_) => 0,
        }
    }

    /// Cursor offset when positioned at the "end" of this segment
    fn end_offset(&self) -> usize {
        match self {
            Segment::Text(s) => s.len(),
            Segment::Attachment(_) => 0,
        }
    }

    /// Display string length (for calculating wrapped positions)
    fn display_len(&self) -> usize {
        match self {
            Segment::Text(s) => s.len(),
            Segment::Attachment(a) => a.display().len(),
        }
    }

    /// Get display representation
    fn display(&self) -> String {
        match self {
            Segment::Text(s) => s.clone(),
            Segment::Attachment(a) => a.display(),
        }
    }

    /// Get expanded content (full attachment content for submit)
    fn expanded(&self) -> String {
        match self {
            Segment::Text(s) => s.clone(),
            Segment::Attachment(a) => a.expanded(),
        }
    }

    /// Get text content only (None for attachments)
    fn text_content(&self) -> Option<&str> {
        match self {
            Segment::Text(s) => Some(s),
            Segment::Attachment(_) => None,
        }
    }

    /// Check if this is a text segment
    fn is_text(&self) -> bool {
        matches!(self, Segment::Text(_))
    }

    /// Check if this is empty (empty text or always false for attachments)
    fn is_empty(&self) -> bool {
        match self {
            Segment::Text(s) => s.is_empty(),
            Segment::Attachment(_) => false,
        }
    }

    /// Get previous cursor offset within this segment, or None if at start
    fn prev_cursor_offset(&self, current: usize) -> Option<usize> {
        match self {
            Segment::Text(s) if current > 0 => {
                let mut new_pos = current - 1;
                while !s.is_char_boundary(new_pos) && new_pos > 0 {
                    new_pos -= 1;
                }
                Some(new_pos)
            }
            _ => None,
        }
    }

    /// Get next cursor offset within this segment, or None if at end
    fn next_cursor_offset(&self, current: usize) -> Option<usize> {
        match self {
            Segment::Text(s) if current < s.len() => {
                let mut new_pos = current + 1;
                while !s.is_char_boundary(new_pos) && new_pos < s.len() {
                    new_pos += 1;
                }
                Some(new_pos)
            }
            _ => None,
        }
    }

    /// Delete character before offset, returns new offset if successful
    fn delete_char_before(&mut self, offset: usize) -> Option<usize> {
        match self {
            Segment::Text(s) if offset > 0 => {
                let mut new_pos = offset - 1;
                while !s.is_char_boundary(new_pos) && new_pos > 0 {
                    new_pos -= 1;
                }
                s.drain(new_pos..offset);
                Some(new_pos)
            }
            _ => None,
        }
    }

    /// Take ownership of text content, leaving empty string
    fn take_text(&mut self) -> String {
        match self {
            Segment::Text(s) => std::mem::take(s),
            Segment::Attachment(_) => String::new(),
        }
    }

    /// Append text to this segment (no-op for attachments)
    fn push_str(&mut self, text: &str) {
        if let Segment::Text(s) = self {
            s.push_str(text);
        }
    }

    /// Split text at offset, returning the after portion. No-op for attachments.
    fn split_off(&mut self, offset: usize) -> String {
        match self {
            Segment::Text(s) if offset < s.len() => {
                let after = s[offset..].to_string();
                s.truncate(offset);
                after
            }
            _ => String::new(),
        }
    }
}

/// Input box widget state
#[derive(Debug, Clone)]
pub struct InputBox {
    segments: Vec<Segment>,
    cursor_seg: usize,
    cursor_offset: usize,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl InputBox {
    pub fn new() -> Self {
        Self {
            segments: vec![Segment::Text(String::new())],
            cursor_seg: 0,
            cursor_offset: 0,
            history: Vec::new(),
            history_index: None,
        }
    }

    /// Ensure cursor is on a text segment, creating one if needed
    fn ensure_text_segment(&mut self) {
        if !self.segments[self.cursor_seg].is_text() {
            // If on attachment, move to/create next text segment
            if self.cursor_seg + 1 >= self.segments.len() {
                self.segments.push(Segment::Text(String::new()));
            }
            self.cursor_seg += 1;
            self.cursor_offset = 0;
        }
    }

    /// Get display string (text + pill labels) for wrapping
    fn display_string(&self) -> String {
        self.segments.iter().map(|seg| seg.display()).collect()
    }

    /// Get expanded string (text + full attachment content) for submit
    fn expanded_string(&self) -> String {
        self.segments.iter().map(|seg| seg.expanded()).collect()
    }

    /// Get typed text only (for tab completion)
    pub fn content(&self) -> String {
        self.segments
            .iter()
            .filter_map(|seg| seg.text_content().map(str::to_owned))
            .collect()
    }

    /// Check if input is empty (no text, no attachments)
    pub fn is_empty(&self) -> bool {
        self.segments.iter().all(|seg| seg.is_empty())
    }

    /// Set the content and move cursor to end (replaces all with single text)
    pub fn set_content(&mut self, content: &str) {
        self.segments = vec![Segment::Text(content.to_string())];
        self.cursor_seg = 0;
        self.cursor_offset = content.len();
    }

    /// Calculate required height for the input box given a width
    pub fn required_height(&self, width: u16) -> u16 {
        let inner_width = width.saturating_sub(2) as usize;
        if inner_width == 0 {
            return 5;
        }
        let wrapped = wrap_text(&self.display_string(), inner_width);
        (wrapped.len() as u16 + 2).max(5)
    }

    /// Insert a character at the cursor position
    pub fn insert_char(&mut self, c: char) {
        self.ensure_text_segment();
        if let Segment::Text(s) = &mut self.segments[self.cursor_seg] {
            s.insert(self.cursor_offset, c);
            self.cursor_offset += c.len_utf8();
        }
    }

    /// Delete the character before the cursor
    pub fn delete_char(&mut self) {
        // Try to delete within current segment
        if let Some(new_offset) = self.segments[self.cursor_seg].delete_char_before(self.cursor_offset) {
            self.cursor_offset = new_offset;
            return;
        }

        // At start of segment - behavior depends on segment type
        if !self.segments[self.cursor_seg].is_text() {
            // On Attachment: delete the attachment itself
            self.segments.remove(self.cursor_seg);
            if self.cursor_seg > 0 {
                self.cursor_seg -= 1;
                self.cursor_offset = self.segments[self.cursor_seg].end_offset();
            } else {
                if self.segments.is_empty() {
                    self.segments.push(Segment::Text(String::new()));
                }
                self.cursor_offset = 0;
            }
            return;
        }

        // On Text at offset 0: delete what's before
        if self.cursor_seg == 0 {
            return; // Nothing before
        }

        let prev = self.cursor_seg - 1;
        if self.segments[prev].is_text() {
            // Merge with previous text segment
            let current_text = self.segments[self.cursor_seg].take_text();
            let prev_len = self.segments[prev].len();
            self.segments[prev].push_str(&current_text);
            self.segments.remove(self.cursor_seg);
            self.cursor_seg = prev;
            self.cursor_offset = prev_len;
        } else {
            // Previous is Attachment: just remove it
            self.segments.remove(prev);
            self.cursor_seg -= 1;
        }
    }

    /// Move cursor left
    pub fn move_cursor_left(&mut self) {
        if let Some(new_offset) = self.segments[self.cursor_seg].prev_cursor_offset(self.cursor_offset) {
            self.cursor_offset = new_offset;
        } else if self.cursor_seg > 0 {
            self.cursor_seg -= 1;
            self.cursor_offset = self.segments[self.cursor_seg].end_offset();
        }
    }

    /// Move cursor right
    pub fn move_cursor_right(&mut self) {
        if let Some(new_offset) = self.segments[self.cursor_seg].next_cursor_offset(self.cursor_offset) {
            self.cursor_offset = new_offset;
        } else if self.cursor_seg + 1 < self.segments.len() {
            self.cursor_seg += 1;
            self.cursor_offset = 0;
        }
    }

    /// Move cursor to start
    pub fn move_cursor_start(&mut self) {
        self.cursor_seg = 0;
        self.cursor_offset = 0;
    }

    /// Move cursor to end
    pub fn move_cursor_end(&mut self) {
        self.cursor_seg = self.segments.len() - 1;
        self.cursor_offset = self.segments[self.cursor_seg].end_offset();
    }

    /// Insert a newline
    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Add attachment at cursor position
    pub fn add_attachment(&mut self, attachment: Attachment) {
        let seg = &self.segments[self.cursor_seg];
        
        // At start of text segment: insert before
        if seg.is_text() && self.cursor_offset == 0 {
            self.segments.insert(self.cursor_seg, Segment::Attachment(attachment));
            self.cursor_seg += 1;
            return;
        }

        // In middle of text: split first
        let after = self.segments[self.cursor_seg].split_off(self.cursor_offset);

        // Insert attachment after current segment, then empty text for cursor
        self.segments.insert(self.cursor_seg + 1, Segment::Attachment(attachment));
        self.segments.insert(self.cursor_seg + 2, Segment::Text(after));
        self.cursor_seg += 2;
        self.cursor_offset = 0;
    }

    /// Clear the input
    pub fn clear(&mut self) {
        self.segments = vec![Segment::Text(String::new())];
        self.cursor_seg = 0;
        self.cursor_offset = 0;
        self.history_index = None;
    }

    /// Submit the current content and add to history
    pub fn submit(&mut self) -> String {
        let content = self.expanded_string();
        let display = self.display_string();
        
        self.segments = vec![Segment::Text(String::new())];
        self.cursor_seg = 0;
        self.cursor_offset = 0;
        self.history_index = None;

        if !display.trim().is_empty() {
            self.history.push(display);
        }

        content
    }

    /// Update the IDE selection (replaces any existing, inserts at front if new)
    /// Pass None to clear the IDE selection.
    pub fn set_ide_selection(&mut self, attachment: Option<Attachment>) {
        // Find existing IDE selection
        let existing_idx = self.segments.iter().position(|seg| {
            matches!(seg, Segment::Attachment(a) if matches!(a.kind, AttachmentKind::IdeSelection { .. }))
        });

        match (existing_idx, attachment) {
            // Update existing
            (Some(idx), Some(new_attachment)) => {
                self.segments[idx] = Segment::Attachment(new_attachment);
            }
            // Remove existing
            (Some(idx), None) => {
                self.segments.remove(idx);
                if self.cursor_seg > idx {
                    self.cursor_seg -= 1;
                } else if self.cursor_seg == idx {
                    self.cursor_seg = 0;
                    self.cursor_offset = 0;
                }
                // Ensure we have at least one text segment
                if self.segments.is_empty() {
                    self.segments.push(Segment::Text(String::new()));
                }
            }
            // Insert new at front
            (None, Some(new_attachment)) => {
                self.segments.insert(0, Segment::Attachment(new_attachment));
                self.cursor_seg += 1;
            }
            // Nothing to do
            (None, None) => {}
        }
    }

    /// Navigate to previous history item
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }

        let new_index = match self.history_index {
            Some(0) => 0,
            Some(i) => i - 1,
            None => self.history.len() - 1,
        };

        self.history_index = Some(new_index);
        self.set_content(&self.history[new_index].clone());
    }

    /// Navigate to next history item
    pub fn history_next(&mut self) {
        if self.history.is_empty() {
            return;
        }

        match self.history_index {
            Some(i) if i < self.history.len() - 1 => {
                self.history_index = Some(i + 1);
                self.set_content(&self.history[i + 1].clone());
            }
            Some(_) => {
                self.history_index = None;
                self.clear();
            }
            None => {}
        }
    }

    /// Get the segments for rendering
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Get cursor position as (segment index, offset within segment)
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_seg, self.cursor_offset)
    }

    /// Render the input box with model name as title and usage display
    pub fn widget<'a>(
        &'a self,
        model: &'a str,
        context_tokens: u32,
    ) -> InputBoxWidget<'a> {
        InputBoxWidget {
            state: self,
            model,
            context_tokens,
        }
    }
}

impl Default for InputBox {
    fn default() -> Self {
        Self::new()
    }
}

/// Input box widget for rendering
pub struct InputBoxWidget<'a> {
    state: &'a InputBox,
    model: &'a str,
    /// Current context window size in tokens
    context_tokens: u32,
}

impl Widget for InputBoxWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Build the usage string for right title
        let usage_title = format!(" {} ", format_tokens(self.context_tokens));

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(format!(" {} ", self.model))
            .title_top(Line::from(usage_title).right_aligned());

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let width = inner.width as usize;
        
        // Build display string and track segment boundaries for cursor
        let display = self.state.display_string();
        let wrapped_lines = wrap_text(&display, width);

        // Render content
        let paragraph = if self.state.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                "Type your message here...",
                Style::default().fg(Color::DarkGray),
            )))
        } else {
            // Build lines with styled spans for attachments
            let mut lines: Vec<Line> = Vec::new();
            let mut current_line_spans: Vec<Span> = Vec::new();
            let mut char_count = 0usize;
            let mut line_idx = 0;
            
            for seg in self.state.segments() {
                let (text, style) = match seg {
                    Segment::Text(s) => (s.clone(), Style::default()),
                    Segment::Attachment(a) => (
                        a.display(),
                        Style::default().bg(Color::DarkGray).fg(Color::White),
                    ),
                };
                
                for _ch in text.chars() {
                    // Check if we've moved to next wrapped line
                    while line_idx < wrapped_lines.len() {
                        let line_len = wrapped_lines[line_idx].chars().count();
                        if char_count < line_len + lines.iter().map(|l: &Line| l.width()).sum::<usize>() {
                            break;
                        }
                        // Finish current line
                        if !current_line_spans.is_empty() {
                            lines.push(Line::from(std::mem::take(&mut current_line_spans)));
                        }
                        line_idx += 1;
                    }
                    
                    // Add char to current span (simplified - just rebuild from wrapped)
                    char_count += 1;
                }
                
                current_line_spans.push(Span::styled(text, style));
            }
            
            if !current_line_spans.is_empty() {
                lines.push(Line::from(current_line_spans));
            }
            
            // Simpler approach: just use wrapped lines for now, attachments show as text
            let lines: Vec<Line> = wrapped_lines.iter().map(|s| Line::from(s.as_str())).collect();
            Paragraph::new(lines)
        };

        paragraph.render(inner, buf);

        // Calculate cursor byte position within display string
        let mut cursor_byte_pos = 0usize;
        let (cursor_seg, cursor_offset) = self.state.cursor();
        for (i, seg) in self.state.segments().iter().enumerate() {
            if i == cursor_seg {
                // For cursor segment: Text uses offset, Attachment uses full display
                cursor_byte_pos += if seg.is_text() { cursor_offset } else { seg.display_len() };
                break;
            }
            cursor_byte_pos += seg.display_len();
        }

        let (cursor_x, cursor_y) = cursor_position_in_wrapped(
            &display,
            cursor_byte_pos,
            &wrapped_lines,
        );

        if cursor_y < inner.height as usize {
            let x = inner.x + cursor_x as u16;
            let y = inner.y + cursor_y as u16;

            if x < inner.x + inner.width && y < inner.y + inner.height {
                buf[(x, y)].set_style(Style::default().bg(Color::White).fg(Color::Black));
            }
        }
    }
}

/// Wrap text into lines, handling explicit newlines
fn wrap_text(content: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![content.to_string()];
    }
    
    let mut result = Vec::new();
    for paragraph in content.split('\n') {
        if paragraph.is_empty() {
            result.push(String::new());
        } else {
            for line in wrap(paragraph, width) {
                result.push(line.into_owned());
            }
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Calculate cursor (x, y) position within wrapped lines
fn cursor_position_in_wrapped(content: &str, byte_pos: usize, wrapped_lines: &[String]) -> (usize, usize) {
    let text_before_cursor = &content[..byte_pos];
    
    // Count trailing spaces that textwrap might have trimmed
    let trailing_spaces = text_before_cursor.chars().rev().take_while(|&c| c == ' ').count();
    
    // Count how many characters (not bytes) before cursor
    let chars_before: usize = text_before_cursor.chars().count();
    
    let mut chars_consumed = 0usize;
    for (line_idx, line) in wrapped_lines.iter().enumerate() {
        let line_chars = line.chars().count();
        
        // Check if cursor is on this line
        if chars_consumed + line_chars >= chars_before - trailing_spaces {
            let col = (chars_before - trailing_spaces) - chars_consumed;
            // Get display width of the portion before cursor on this line
            let prefix: String = line.chars().take(col).collect();
            let cursor_x = prefix.width() + trailing_spaces;
            return (cursor_x, line_idx);
        }
        
        chars_consumed += line_chars;
    }
    
    // Cursor at end
    let last_line_width = wrapped_lines.last().map(|s| s.width()).unwrap_or(0);
    (last_line_width + trailing_spaces, wrapped_lines.len().saturating_sub(1))
}

#[cfg(test)]
mod tests {
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
}
