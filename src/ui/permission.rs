//! Permission dialog and handling

use async_trait::async_trait;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};

/// Risk level for a tool operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    pub fn symbol(&self) -> &str {
        match self {
            RiskLevel::Low => "ℹ️ ",
            RiskLevel::Medium => "⚠️ ",
            RiskLevel::High => "🔴",
        }
    }

    pub fn text(&self) -> &str {
        match self {
            RiskLevel::Low => "LOW",
            RiskLevel::Medium => "MEDIUM",
            RiskLevel::High => "HIGH",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            RiskLevel::Low => Color::Green,
            RiskLevel::Medium => Color::Yellow,
            RiskLevel::High => Color::Red,
        }
    }
}

/// Permission request from a tool
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool_name: String,
    pub params: serde_json::Value,
    pub description: String,
    pub risk_level: RiskLevel,
}

/// User's response to a permission request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionResponse {
    Allow,
    AllowOnce,
    AllowForSession,
    Deny,
}

/// Trait for handling permission requests
#[async_trait]
pub trait PermissionHandler: Send + Sync {
    async fn request_permission(&self, request: PermissionRequest) -> PermissionResponse;
}

/// Permission dialog widget state
#[derive(Debug, Clone)]
pub struct PermissionDialog {
    request: PermissionRequest,
    selected_action: usize,
}

impl PermissionDialog {
    pub fn new(request: PermissionRequest) -> Self {
        Self {
            request,
            selected_action: 0,
        }
    }

    /// Get the current request
    pub fn request(&self) -> &PermissionRequest {
        &self.request
    }

    /// Select the next action
    pub fn next_action(&mut self) {
        self.selected_action = (self.selected_action + 1) % 3;
    }

    /// Select the previous action
    pub fn prev_action(&mut self) {
        self.selected_action = if self.selected_action == 0 {
            2
        } else {
            self.selected_action - 1
        };
    }

    /// Get the selected response
    pub fn selected_response(&self) -> PermissionResponse {
        match self.selected_action {
            0 => PermissionResponse::Allow,
            1 => PermissionResponse::AllowForSession,
            2 => PermissionResponse::Deny,
            _ => PermissionResponse::Allow,
        }
    }

    /// Render the dialog
    pub fn widget(&self) -> PermissionDialogWidget<'_> {
        PermissionDialogWidget { state: self }
    }
}

/// Permission dialog widget for rendering
pub struct PermissionDialogWidget<'a> {
    state: &'a PermissionDialog,
}

impl PermissionDialogWidget<'_> {
    fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
        let popup_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ])
            .split(r);

        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ])
            .split(popup_layout[1])[1]
    }
}

impl Widget for PermissionDialogWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let dialog_area = Self::centered_rect(60, 50, area);

        // Clear the background
        Clear.render(dialog_area, buf);

        // Create the dialog block
        let block = Block::default()
            .title(" Tool Execution Request ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Black));

        let inner = block.inner(dialog_area);
        block.render(dialog_area, buf);

        // Split inner area
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Tool info
                Constraint::Min(5),    // Parameters
                Constraint::Length(3), // Actions
            ])
            .margin(1)
            .split(inner);

        // Tool info
        let tool_info = vec![
            Line::from(vec![
                Span::styled("Tool: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    &self.state.request.tool_name,
                    Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Risk: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format!("{} {}", self.state.request.risk_level.symbol(), self.state.request.risk_level.text()),
                    Style::default().fg(self.state.request.risk_level.color()),
                ),
            ]),
        ];
        Paragraph::new(tool_info).render(chunks[0], buf);

        // Parameters
        let params_text = format!(
            "{}\n\n{}",
            self.state.request.description,
            serde_json::to_string_pretty(&self.state.request.params)
                .unwrap_or_else(|_| "{}".to_string())
        );
        let params = Paragraph::new(params_text)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title(" Details ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
        params.render(chunks[1], buf);

        // Actions
        let actions = vec![
            ("y", "Allow", self.state.selected_action == 0),
            ("a", "Allow for session", self.state.selected_action == 1),
            ("n", "Deny", self.state.selected_action == 2),
        ];

        let action_spans: Vec<Span> = actions
            .iter()
            .flat_map(|(key, label, selected)| {
                let style = if *selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };

                vec![
                    Span::styled(format!("[{}] ", key), Style::default().fg(Color::Yellow)),
                    Span::styled(*label, style),
                    Span::raw("   "),
                ]
            })
            .collect();

        let actions_line = Line::from(action_spans);
        let actions_paragraph = Paragraph::new(actions_line).alignment(Alignment::Center);
        actions_paragraph.render(chunks[2], buf);
    }
}

/// Simple permission handler that always allows (for testing)
pub struct AlwaysAllowHandler;

#[async_trait]
impl PermissionHandler for AlwaysAllowHandler {
    async fn request_permission(&self, _request: PermissionRequest) -> PermissionResponse {
        PermissionResponse::Allow
    }
}

/// Simple permission handler that always denies (for testing)
pub struct AlwaysDenyHandler;

#[async_trait]
impl PermissionHandler for AlwaysDenyHandler {
    async fn request_permission(&self, _request: PermissionRequest) -> PermissionResponse {
        PermissionResponse::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_risk_level() {
        assert_eq!(RiskLevel::Low.text(), "LOW");
        assert_eq!(RiskLevel::High.color(), Color::Red);
    }

    #[test]
    fn test_permission_dialog_navigation() {
        let request = PermissionRequest {
            tool_name: "test".to_string(),
            params: serde_json::json!({}),
            description: "Test".to_string(),
            risk_level: RiskLevel::Low,
        };

        let mut dialog = PermissionDialog::new(request);
        assert_eq!(dialog.selected_response(), PermissionResponse::Allow);

        dialog.next_action();
        assert_eq!(dialog.selected_response(), PermissionResponse::AllowForSession);

        dialog.next_action();
        assert_eq!(dialog.selected_response(), PermissionResponse::Deny);

        dialog.next_action();
        assert_eq!(dialog.selected_response(), PermissionResponse::Allow);
    }
}
