//! Chat view component

use crate::transcript::{Role, Transcript, Turn};
use ratatui::{
    buffer::Buffer,
    layout::{Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, StatefulWidget, Widget},
};
use tui_scrollview::{ScrollView, ScrollViewState};

/// Chat view state - renders from a Transcript
#[derive(Debug, Default)]
pub struct ChatView {
    /// Scroll state managed by tui-scrollview
    pub scroll_state: ScrollViewState,
    /// Track if we should auto-scroll to bottom
    auto_scroll: bool,
}

impl ChatView {
    pub fn new() -> Self {
        Self {
            scroll_state: ScrollViewState::new(),
            auto_scroll: true,
        }
    }

    /// Scroll up by one line
    pub fn scroll_up(&mut self) {
        self.auto_scroll = false;
        self.scroll_state.scroll_up();
    }

    /// Scroll down by one line
    pub fn scroll_down(&mut self) {
        self.scroll_state.scroll_down();
    }

    /// Scroll up by a page
    pub fn page_up(&mut self, page_size: usize) {
        self.auto_scroll = false;
        for _ in 0..page_size {
            self.scroll_state.scroll_up();
        }
    }

    /// Scroll down by a page
    pub fn page_down(&mut self, page_size: usize) {
        for _ in 0..page_size {
            self.scroll_state.scroll_down();
        }
    }

    /// Enable auto-scroll
    pub fn enable_auto_scroll(&mut self) {
        self.auto_scroll = true;
    }

    /// Create a widget that renders the given transcript
    pub fn widget<'a>(&'a mut self, transcript: &'a Transcript) -> ChatViewWidget<'a> {
        ChatViewWidget {
            view: self,
            transcript,
        }
    }
}

/// Chat view widget for rendering
pub struct ChatViewWidget<'a> {
    view: &'a mut ChatView,
    transcript: &'a Transcript,
}

impl ChatViewWidget<'_> {
    /// Returns (header_lines, content_lines) for a turn
    fn render_turn(turn: &Turn, width: u16) -> (Vec<Line<'_>>, Vec<Line<'_>>) {
        // Role header
        let (role_text, role_style) = match turn.role {
            Role::User => (
                "You",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::Assistant => (
                "Codey",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::System => (
                "System",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        };

        let header = vec![
            Span::styled(role_text, role_style),
            Span::styled(
                format!(" ({})", turn.timestamp.format("%H:%M:%S")),
                Style::default().fg(Color::DarkGray),
            ),
        ];
        
        let header_lines = vec![Line::from(header)];
        let content_lines = turn.render(width);
        
        (header_lines, content_lines)
    }
}

impl Widget for ChatViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let inner = area;
        // Subtract scrollbar width
        let content_width = inner.width.saturating_sub(2);

        // First pass: calculate total height and collect rendered turns
        let mut total_height: u16 = 0;
        let mut rendered_turns: Vec<(Vec<Line>, Vec<Line>)> = Vec::new();
        
        for turn in self.transcript.turns() {
            let (header, content) = Self::render_turn(turn, content_width.saturating_sub(2));
            // header + content block + separator
            let turn_height = header.len() as u16 + content.len() as u16 + 1;
            total_height += turn_height;
            rendered_turns.push((header, content));
        }

        let content_height = total_height.max(1);

        // If auto-scroll is enabled, calculate offset to show bottom
        if self.view.auto_scroll {
            let max_offset = content_height.saturating_sub(inner.height);
            self.view.scroll_state.set_offset(ratatui::layout::Position::new(0, max_offset));
        }

        // Create scroll view
        let mut scroll_view = ScrollView::new(Size::new(content_width, content_height))
            .vertical_scrollbar_visibility(tui_scrollview::ScrollbarVisibility::Always)
            .horizontal_scrollbar_visibility(tui_scrollview::ScrollbarVisibility::Never);

        // Second pass: render each turn at its position
        let mut y_offset: u16 = 0;
        for (header, content) in rendered_turns {
            // Render header
            let header_paragraph = Paragraph::new(header);
            scroll_view.render_widget(
                header_paragraph,
                Rect::new(0, y_offset, content_width, 1),
            );
            y_offset += 1;

            // Render content in a styled block
            let content_height = content.len() as u16;
            if content_height > 0 {
                let content_block = Block::default()
                    .style(Style::default().bg(Color::Indexed(234)))
                    .padding(Padding::left(1));
                let content_paragraph = Paragraph::new(content).block(content_block);
                scroll_view.render_widget(
                    content_paragraph,
                    Rect::new(0, y_offset, content_width, content_height),
                );
                y_offset += content_height;
            }

            // Separator
            y_offset += 1;
        }

        // Render the scroll view
        scroll_view.render(inner, buf, &mut self.view.scroll_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_view_scroll() {
        let mut chat = ChatView::new();
        assert!(chat.auto_scroll);

        chat.scroll_up();
        assert!(!chat.auto_scroll);

        chat.enable_auto_scroll();
        assert!(chat.auto_scroll);
    }
}
