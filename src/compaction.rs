//! Context compaction for managing token usage
//!
//! When the conversation context exceeds a threshold, this module handles
//! asking the agent to summarize the conversation for continuation in a
//! new transcript.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};

use crate::transcript::{Block, BlockType, Status};
use crate::impl_base_block;

/// The prompt sent to the agent to generate a compaction summary
pub const COMPACTION_PROMPT: &str = r#"The conversation context is getting large and needs to be compacted.

Please provide a comprehensive summary of our conversation so far in markdown format. Include:

1. **What was accomplished** - Main tasks and changes completed
2. **What still needs to be done** - Remaining tasks or open questions
3. **Key project information** - Important facts about the project (architecture, patterns, gotchas)
4. **Relevant files** - Files most relevant to the current work with brief descriptions

Be thorough but concise - this summary will seed a fresh conversation context."#;

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

        match self.status {
            Status::Pending | Status::Running => {
                lines.push(Line::from(vec![
                    Span::styled("📋 ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        "Compacting context",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));

                if self.status == Status::Running {
                    lines.push(Line::from(Span::styled(
                        "  Generating summary...",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            Status::Complete => {
                lines.push(Line::from(vec![
                    Span::styled("📋 ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        "Context Compacted",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));

                lines.push(Line::from(""));

                // Render the summary using markdown
                let skin = ratskin::RatSkin::default();
                let parsed = ratskin::RatSkin::parse_text(&self.text);
                for line in skin.parse(parsed, width) {
                    lines.push(line);
                }
            }
            _ => {
                lines.push(Line::from(vec![
                    Span::styled("📋 ", Style::default().fg(Color::Red)),
                    Span::styled(
                        "Context compaction failed",
                        Style::default().fg(Color::Red),
                    ),
                ]));
            }
        }

        lines
    }
}
