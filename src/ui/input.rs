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
        background_tasks: usize,
        agent_active: bool,
    ) -> InputBoxWidget<'a> {
        InputBoxWidget {
            state: self,
            model,
            context_tokens,
            background_tasks,
            agent_active,
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
    /// Number of running background tasks
    background_tasks: usize,
    /// Whether the agent is actively processing (streaming, tool execution)
    agent_active: bool,
}

impl Widget for InputBoxWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Build the usage string for right title
        let usage_title = format!(" {} ", format_tokens(self.context_tokens));
        
        // Build model title with background indicator
        let model_title = if self.background_tasks > 0 {
            format!(" * {} ", self.model)
        } else {
            format!(" {} ", self.model)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(model_title)
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
                if self.agent_active {
                    // Dimmed reversed cursor when agent is busy
                    buf[(x, y)].set_style(Style::default().bg(Color::DarkGray).fg(Color::Black));
                } else {
                    // Bright reversed cursor when ready for input
                    buf[(x, y)].set_style(Style::default().bg(Color::White).fg(Color::Black));
                }
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
#[path = "input_tests.rs"]
mod tests;
