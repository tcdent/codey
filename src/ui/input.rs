//! Input box component

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

/// Input box widget state
#[derive(Debug, Clone)]
pub struct InputBox {
    content: String,
    cursor_position: usize,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl InputBox {
    pub fn new() -> Self {
        Self {
            content: String::new(),
            cursor_position: 0,
            history: Vec::new(),
            history_index: None,
        }
    }

    /// Get the current content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get the cursor position
    pub fn cursor_position(&self) -> usize {
        self.cursor_position
    }

    /// Insert a character at the cursor position
    pub fn insert_char(&mut self, c: char) {
        self.content.insert(self.cursor_position, c);
        self.cursor_position += c.len_utf8();
    }

    /// Delete the character before the cursor
    pub fn delete_char(&mut self) {
        if self.cursor_position == 0 {
            return;
        }

        // Find the previous character boundary
        let mut new_pos = self.cursor_position - 1;
        while !self.content.is_char_boundary(new_pos) && new_pos > 0 {
            new_pos -= 1;
        }

        self.content.drain(new_pos..self.cursor_position);
        self.cursor_position = new_pos;
    }

    /// Delete the character at the cursor
    pub fn delete_char_forward(&mut self) {
        if self.cursor_position >= self.content.len() {
            return;
        }

        // Find the next character boundary
        let mut end_pos = self.cursor_position + 1;
        while !self.content.is_char_boundary(end_pos) && end_pos < self.content.len() {
            end_pos += 1;
        }

        self.content.drain(self.cursor_position..end_pos);
    }

    /// Move cursor left
    pub fn move_cursor_left(&mut self) {
        if self.cursor_position == 0 {
            return;
        }

        let mut new_pos = self.cursor_position - 1;
        while !self.content.is_char_boundary(new_pos) && new_pos > 0 {
            new_pos -= 1;
        }
        self.cursor_position = new_pos;
    }

    /// Move cursor right
    pub fn move_cursor_right(&mut self) {
        if self.cursor_position >= self.content.len() {
            return;
        }

        let mut new_pos = self.cursor_position + 1;
        while !self.content.is_char_boundary(new_pos) && new_pos < self.content.len() {
            new_pos += 1;
        }
        self.cursor_position = new_pos;
    }

    /// Move cursor to start
    pub fn move_cursor_start(&mut self) {
        self.cursor_position = 0;
    }

    /// Move cursor to end
    pub fn move_cursor_end(&mut self) {
        self.cursor_position = self.content.len();
    }

    /// Insert a newline
    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Clear the input
    pub fn clear(&mut self) {
        self.content.clear();
        self.cursor_position = 0;
        self.history_index = None;
    }

    /// Submit the current content and add to history
    pub fn submit(&mut self) -> String {
        let content = std::mem::take(&mut self.content);
        self.cursor_position = 0;
        self.history_index = None;

        if !content.trim().is_empty() {
            self.history.push(content.clone());
        }

        content
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
        self.content = self.history[new_index].clone();
        self.cursor_position = self.content.len();
    }

    /// Navigate to next history item
    pub fn history_next(&mut self) {
        if self.history.is_empty() {
            return;
        }

        match self.history_index {
            Some(i) if i < self.history.len() - 1 => {
                self.history_index = Some(i + 1);
                self.content = self.history[i + 1].clone();
                self.cursor_position = self.content.len();
            }
            Some(_) => {
                // At the end of history, clear
                self.history_index = None;
                self.clear();
            }
            None => {}
        }
    }

    /// Render the input box
    pub fn widget(&self) -> InputBoxWidget<'_> {
        InputBoxWidget { state: self }
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
}

impl Widget for InputBoxWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Input (Enter to send, Shift+Enter for newline) ");

        let inner = block.inner(area);
        block.render(area, buf);

        // Render content
        let content = if self.state.content.is_empty() {
            Span::styled(
                "Type your message here...",
                Style::default().fg(Color::DarkGray),
            )
        } else {
            Span::raw(&self.state.content)
        };

        let paragraph = Paragraph::new(Line::from(content)).wrap(Wrap { trim: false });

        paragraph.render(inner, buf);

        // Render cursor
        if inner.width > 0 && inner.height > 0 {
            // Calculate cursor position in the wrapped text
            let cursor_x = self.state.cursor_position % inner.width as usize;
            let cursor_y = self.state.cursor_position / inner.width as usize;

            if cursor_y < inner.height as usize {
                let x = inner.x + cursor_x as u16;
                let y = inner.y + cursor_y as u16;

                if x < inner.x + inner.width && y < inner.y + inner.height {
                    buf[(x, y)].set_style(Style::default().bg(Color::White).fg(Color::Black));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_box_basic() {
        let mut input = InputBox::new();

        input.insert_char('H');
        input.insert_char('i');
        assert_eq!(input.content(), "Hi");
        assert_eq!(input.cursor_position(), 2);

        input.delete_char();
        assert_eq!(input.content(), "H");
        assert_eq!(input.cursor_position(), 1);
    }

    #[test]
    fn test_input_box_cursor_movement() {
        let mut input = InputBox::new();
        input.insert_char('a');
        input.insert_char('b');
        input.insert_char('c');

        input.move_cursor_left();
        assert_eq!(input.cursor_position(), 2);

        input.move_cursor_start();
        assert_eq!(input.cursor_position(), 0);

        input.move_cursor_end();
        assert_eq!(input.cursor_position(), 3);
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
}
