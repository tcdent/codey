//! Context compaction for managing token usage
//!
//! When the conversation context exceeds a threshold, this module handles
//! asking the agent to summarize the conversation for continuation in a
//! new transcript.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};

use crate::impl_base_block;
use crate::transcript::{Block, BlockType, Status};

/// Compaction summary block - shown when context was compacted
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionBlock {
    pub text: String,
    pub status: Status,
}

impl CompactionBlock {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status: Status::Running,
        }
    }
}

#[typetag::serde]
impl Block for CompactionBlock {
    impl_base_block!(BlockType::Compaction);

    fn render(&self, width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        // Header with status
        let (icon, color) = match self.status {
            Status::Pending | Status::Running => ("⚙ ", Color::Yellow),
            Status::Complete => ("✓ ", Color::Cyan),
            _ => ("✗ ", Color::Red),
        };
        
        let title = match self.status {
            Status::Pending | Status::Running => "Compacting context...",
            Status::Complete => "Context Compacted",
            _ => "Context compaction failed",
        };
        
        lines.push(Line::from(vec![
            Span::styled(icon, Style::default().fg(color)),
            Span::styled(
                title,
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Only render text content when complete
        if self.status == Status::Complete && !self.text.is_empty() {
            lines.push(Line::from(""));
            let skin = ratskin::RatSkin::default();
            let parsed = ratskin::RatSkin::parse_text(&self.text);
            for line in skin.parse(parsed, width) {
                lines.push(line);
            }
        }

        lines
    }
}
