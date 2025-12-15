//! Chat view component

use crate::message::{Message, Role, Status, Transcript};
use ratatui::{
    buffer::Buffer,
    layout::{Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
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
    fn render_message(msg: &Message, width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        // Role header
        let (role_text, role_style) = match msg.role {
            Role::User => (
                "You",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Role::Assistant => (
                "Claude",
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

        // Status indicator for non-complete messages
        let status_span = match msg.status {
            Status::Pending => Some(Span::styled(" (queued)", Style::default().fg(Color::Yellow))),
            Status::Running => {
                Some(Span::styled(" (sending...)", Style::default().fg(Color::Blue)))
            }
            Status::Error => Some(Span::styled(" (error)", Style::default().fg(Color::Red))),
            Status::Success | Status::Denied => None,
        };

        let mut header = vec![
            Span::styled(role_text, role_style),
            Span::styled(
                format!(" ({})", msg.timestamp.format("%H:%M:%S")),
                Style::default().fg(Color::DarkGray),
            ),
        ];
        if let Some(status) = status_span {
            header.push(status);
        }
        lines.push(Line::from(header));

        // Render all content blocks via trait
        lines.extend(msg.render(width));

        // Separator
        lines.push(Line::from(""));

        lines
    }
}

impl Widget for ChatViewWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray))
            .title(" Chat ");

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Build all lines from transcript
        let mut all_lines: Vec<Line> = Vec::new();
        // inner accounts for the border, subtract scrollbar (1) + small margin (1)
        let content_width = inner.width.saturating_sub(2);

        for msg in self.transcript.messages() {
            all_lines.extend(Self::render_message(msg, content_width));
        }

        // TODO: This scroll/height calculation is brittle. We manually track line counts
        // and calculate scroll offsets because tui-scrollview's scroll_to_bottom() doesn't
        // work reliably when content size changes between frames. Revisit when tui-scrollview
        // stabilizes or consider alternative approaches (e.g., tracking content height
        // separately, or using a different scrolling widget).
        
        // Calculate content size for ScrollView
        // Markdown renderer already handles wrapping at content_width
        // Ensure minimum height to avoid scroll issues
        let content_height = (all_lines.len() as u16).max(1);

        // If auto-scroll is enabled, calculate offset to show bottom
        if self.view.auto_scroll {
            let max_offset = content_height.saturating_sub(inner.height);
            self.view.scroll_state.set_offset(ratatui::layout::Position::new(0, max_offset));
        }

        // Create scroll view with content size, always show vertical scrollbar
        let mut scroll_view = ScrollView::new(Size::new(content_width, content_height))
            .vertical_scrollbar_visibility(tui_scrollview::ScrollbarVisibility::Always)
            .horizontal_scrollbar_visibility(tui_scrollview::ScrollbarVisibility::Never);

        // Render paragraph into scroll view's buffer (no wrap - markdown already wrapped)
        let paragraph = Paragraph::new(all_lines);
        scroll_view.render_widget(paragraph, Rect::new(0, 0, content_width, content_height));

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
