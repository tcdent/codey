//! Chat view component

use crate::llm::Role;
use chrono::{DateTime, Utc};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget, Wrap},
};

/// A displayable message in the chat
#[derive(Debug, Clone)]
pub struct DisplayMessage {
    pub role: Role,
    pub content: Vec<DisplayContent>,
    pub timestamp: DateTime<Utc>,
}

impl DisplayMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![DisplayContent::Text(text.into())],
            timestamp: Utc::now(),
        }
    }

    pub fn assistant(content: Vec<DisplayContent>) -> Self {
        Self {
            role: Role::Assistant,
            content,
            timestamp: Utc::now(),
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![DisplayContent::Text(text.into())],
            timestamp: Utc::now(),
        }
    }
}

/// Content type for display
#[derive(Debug, Clone)]
pub enum DisplayContent {
    Text(String),
    CodeBlock {
        language: String,
        code: String,
    },
    ToolCall {
        name: String,
        summary: String,
        result: Option<String>,
        is_error: bool,
    },
}

/// Chat view state
#[derive(Debug)]
pub struct ChatView {
    messages: Vec<DisplayMessage>,
    scroll_offset: usize,
    auto_scroll: bool,
    streaming_text: Option<String>,
}

impl ChatView {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
            streaming_text: None,
        }
    }

    /// Add a message to the chat
    pub fn add_message(&mut self, message: DisplayMessage) {
        self.messages.push(message);
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Set streaming text (partial response)
    pub fn set_streaming_text(&mut self, text: Option<String>) {
        self.streaming_text = text;
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Append to streaming text
    pub fn append_streaming_text(&mut self, text: &str) {
        if let Some(ref mut streaming) = self.streaming_text {
            streaming.push_str(text);
        } else {
            self.streaming_text = Some(text.to_string());
        }
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Clear streaming text and optionally add as message
    pub fn finish_streaming(&mut self) -> Option<String> {
        self.streaming_text.take()
    }

    /// Get all messages
    pub fn messages(&self) -> &[DisplayMessage] {
        &self.messages
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
        self.scroll_offset = 0;
        self.streaming_text = None;
    }

    /// Scroll up by one line
    pub fn scroll_up(&mut self) {
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll down by one line
    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    /// Scroll up by a page
    pub fn page_up(&mut self, page_size: usize) {
        self.auto_scroll = false;
        self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
    }

    /// Scroll down by a page
    pub fn page_down(&mut self, page_size: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(page_size);
    }

    /// Scroll to bottom
    pub fn scroll_to_bottom(&mut self) {
        self.auto_scroll = true;
        // Scroll offset will be calculated during render
        self.scroll_offset = usize::MAX;
    }

    /// Enable auto-scroll
    pub fn enable_auto_scroll(&mut self) {
        self.auto_scroll = true;
    }

    /// Render the chat view
    pub fn widget(&self) -> ChatViewWidget<'_> {
        ChatViewWidget { state: self }
    }
}

impl Default for ChatView {
    fn default() -> Self {
        Self::new()
    }
}

/// Chat view widget for rendering
pub struct ChatViewWidget<'a> {
    state: &'a ChatView,
}

impl ChatViewWidget<'_> {
    fn render_message<'a>(msg: &'a DisplayMessage) -> Vec<Line<'a>> {
        let mut lines = Vec::new();

        // Role header
        let (role_text, role_style) = match msg.role {
            Role::User => (
                "You",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Role::Assistant => (
                "Claude",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        };

        lines.push(Line::from(vec![
            Span::styled(role_text, role_style),
            Span::styled(
                format!(" ({})", msg.timestamp.format("%H:%M:%S")),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        // Content
        for content in &msg.content {
            match content {
                DisplayContent::Text(text) => {
                    for line in text.lines() {
                        lines.push(Line::from(Span::raw(line.to_string())));
                    }
                }
                DisplayContent::CodeBlock { language, code } => {
                    lines.push(Line::from(Span::styled(
                        format!("```{}", language),
                        Style::default().fg(Color::Yellow),
                    )));
                    for line in code.lines() {
                        lines.push(Line::from(Span::styled(
                            line.to_string(),
                            Style::default().fg(Color::Gray),
                        )));
                    }
                    lines.push(Line::from(Span::styled(
                        "```",
                        Style::default().fg(Color::Yellow),
                    )));
                }
                DisplayContent::ToolCall {
                    name,
                    summary,
                    result,
                    is_error,
                } => {
                    let icon = if *is_error { "✗" } else { "✓" };
                    let color = if *is_error { Color::Red } else { Color::Green };

                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{} ", icon),
                            Style::default().fg(color),
                        ),
                        Span::styled(
                            format!("{}: ", name),
                            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(summary.clone()),
                    ]));

                    if let Some(result_text) = result {
                        // Show truncated result
                        let preview: String = result_text.lines().take(5).collect::<Vec<_>>().join("\n");
                        lines.push(Line::from(Span::styled(
                            format!("  {}", preview.replace('\n', "\n  ")),
                            Style::default().fg(Color::DarkGray),
                        )));
                        if result_text.lines().count() > 5 {
                            lines.push(Line::from(Span::styled(
                                "  ...",
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                    }
                }
            }
        }

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

        // Build all lines
        let mut all_lines: Vec<Line> = Vec::new();

        for msg in &self.state.messages {
            all_lines.extend(Self::render_message(msg));
        }

        // Add streaming text if present
        if let Some(ref streaming) = self.state.streaming_text {
            all_lines.push(Line::from(vec![
                Span::styled(
                    "Claude",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" (streaming...)", Style::default().fg(Color::DarkGray)),
            ]));
            for line in streaming.lines() {
                all_lines.push(Line::from(Span::raw(line.to_string())));
            }
            // Add blinking cursor
            all_lines.push(Line::from(Span::styled(
                "▌",
                Style::default().fg(Color::Cyan),
            )));
        }

        // Calculate scroll position
        let total_lines = all_lines.len();
        let visible_lines = inner.height as usize;
        let max_scroll = total_lines.saturating_sub(visible_lines);

        let scroll_offset = if self.state.auto_scroll || self.state.scroll_offset >= max_scroll {
            max_scroll
        } else {
            self.state.scroll_offset.min(max_scroll)
        };

        // Create paragraph with scroll
        let text = Text::from(all_lines);
        let paragraph = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset as u16, 0));

        paragraph.render(inner, buf);

        // Render scrollbar if needed
        if total_lines > visible_lines {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
            let mut scrollbar_state = ScrollbarState::new(total_lines)
                .position(scroll_offset);

            let scrollbar_area = Rect {
                x: area.x + area.width - 1,
                y: area.y + 1,
                width: 1,
                height: area.height - 2,
            };

            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_view_add_message() {
        let mut chat = ChatView::new();

        chat.add_message(DisplayMessage::user("Hello"));
        assert_eq!(chat.messages().len(), 1);

        chat.add_message(DisplayMessage::assistant_text("Hi there!"));
        assert_eq!(chat.messages().len(), 2);
    }

    #[test]
    fn test_chat_view_streaming() {
        let mut chat = ChatView::new();

        chat.set_streaming_text(Some("Hello".to_string()));
        chat.append_streaming_text(" world");

        let text = chat.finish_streaming();
        assert_eq!(text, Some("Hello world".to_string()));
    }

    #[test]
    fn test_chat_view_scroll() {
        let mut chat = ChatView::new();

        chat.scroll_up();
        assert!(!chat.auto_scroll);

        chat.scroll_to_bottom();
        assert!(chat.auto_scroll);
    }
}
