//! UI components for the TUI

mod chat;
mod input;
mod markdown;
mod permission;
mod status;

pub use chat::{ChatView, DisplayContent, DisplayMessage};
pub use input::{InputBox, InputMode};
pub use markdown::MarkdownRenderer;
pub use permission::{PermissionDialog, PermissionHandler, PermissionRequest, PermissionResponse, RiskLevel};
pub use status::{ConnectionStatus, StatusBar};

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Calculate the main layout for the application
pub fn main_layout(area: Rect) -> (Rect, Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),      // Status bar
            Constraint::Min(10),        // Chat area
            Constraint::Length(5),      // Input area
        ])
        .split(area);

    (chunks[0], chunks[1], chunks[2])
}
