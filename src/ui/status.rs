//! Status bar component

use crate::llm::Usage;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

/// Connection status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    Disconnected,
    Error(String),
}

impl ConnectionStatus {
    pub fn symbol(&self) -> &str {
        match self {
            ConnectionStatus::Connected => "●",
            ConnectionStatus::Connecting => "◐",
            ConnectionStatus::Disconnected => "○",
            ConnectionStatus::Error(_) => "✗",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            ConnectionStatus::Connected => Color::Green,
            ConnectionStatus::Connecting => Color::Yellow,
            ConnectionStatus::Disconnected => Color::Gray,
            ConnectionStatus::Error(_) => Color::Red,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            ConnectionStatus::Connected => "Connected",
            ConnectionStatus::Connecting => "Connecting...",
            ConnectionStatus::Disconnected => "Disconnected",
            ConnectionStatus::Error(msg) => msg,
        }
    }
}

/// Status bar widget
pub struct StatusBar<'a> {
    app_name: &'a str,
    version: &'a str,
    model: &'a str,
    status: &'a ConnectionStatus,
    usage: Option<Usage>,
    show_tokens: bool,
}

impl<'a> StatusBar<'a> {
    pub fn new(
        app_name: &'a str,
        version: &'a str,
        model: &'a str,
        status: &'a ConnectionStatus,
    ) -> Self {
        Self {
            app_name,
            version,
            model,
            status,
            usage: None,
            show_tokens: true,
        }
    }

    pub fn usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn show_tokens(mut self, show: bool) -> Self {
        self.show_tokens = show;
        self
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style = Style::default().bg(Color::DarkGray).fg(Color::White);

        // Clear the area
        buf.set_style(area, style);

        // Build the status line
        let mut spans = vec![
            Span::styled(
                format!(" {} v{} ", self.app_name, self.version),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("│ "),
            Span::styled(
                format!("Model: {} ", self.model),
                Style::default().fg(Color::White),
            ),
            Span::raw("│ "),
            Span::styled(
                format!("{} ", self.status.symbol()),
                Style::default().fg(self.status.color()),
            ),
            Span::styled(self.status.text(), Style::default().fg(self.status.color())),
        ];

        // Add token usage if available
        if self.show_tokens {
            if let Some(usage) = self.usage {
                spans.push(Span::raw(" │ "));
                spans.push(Span::styled(
                    format!("Tokens: {} in / {} out", usage.input_tokens, usage.output_tokens),
                    Style::default().fg(Color::Gray),
                ));
            }
        }

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_status() {
        assert_eq!(ConnectionStatus::Connected.symbol(), "●");
        assert_eq!(ConnectionStatus::Connected.color(), Color::Green);

        let error = ConnectionStatus::Error("API error".to_string());
        assert_eq!(error.symbol(), "✗");
        assert_eq!(error.text(), "API error");
    }
}
