//! Core types for chat transcript
//!
//! This module contains the types that represent the conversation transcript.
//! A Transcript contains Turns, and each Turn contains Blocks.

use chrono::{DateTime, Utc};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};

/// Unique identifier for a turn
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub usize);

/// Role of the message sender
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// Status of a message, tool, or action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Running,
    Success,
    Error,
    Denied,
}

/// Trait for all blocks in a turn
pub trait Block: Send + Sync {
    /// Render this block to terminal lines with given width for wrapping
    fn render(&self, width: u16) -> Vec<Line<'_>>;

    /// Get the status of this block
    fn status(&self) -> Status;

    /// Set the status of this block
    fn set_status(&mut self, status: Status);

    /// Append text content to this block (for streaming)
    fn append_text(&mut self, _text: &str) {}

    /// Set the result/output of this block
    fn set_result(&mut self, _result: String) {}

    /// TODO: Revisit whether call_id belongs on the trait or should use
    /// a separate lookup mechanism (e.g., HashMap on Turn).
    /// Currently here because tool blocks need to be found by call_id
    /// within a turn, but TextBlock doesn't need it.
    fn call_id(&self) -> Option<&str> {
        None
    }
}

/// Simple text content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBlock {
    pub text: String,
    pub status: Status,
}

impl TextBlock {
    pub fn new(text: impl Into<String>) -> Self {
        Self { 
            text: text.into(),
            status: Status::Success,
        }
    }
}

impl Block for TextBlock {
    fn render(&self, width: u16) -> Vec<Line<'_>> {
        // Use ratskin for markdown rendering
        let skin = ratskin::RatSkin::default();
        let text = ratskin::RatSkin::parse_text(&self.text);
        skin.parse(text, width)
    }

    fn status(&self) -> Status {
        self.status
    }

    fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    fn append_text(&mut self, text: &str) {
        self.text.push_str(text);
    }
}

/// Generic tool content (fallback for tools without specialized display)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolBlock {
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub result: Option<String>,
}

impl ToolBlock {
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            params,
            status: Status::Pending,
            result: None,
        }
    }
}

impl Block for ToolBlock {
    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let (icon, color) = match self.status {
            Status::Pending => ("?", Color::Yellow),
            Status::Running => ("⚙", Color::Blue),
            Status::Success => ("✓", Color::Green),
            Status::Error => ("✗", Color::Red),
            Status::Denied => ("⊘", Color::DarkGray),
        };

        // Tool name with status icon
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", icon), Style::default().fg(color)),
            Span::styled(
                &self.name,
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // Params
        let params_str = serde_json::to_string_pretty(&self.params).unwrap_or_default();
        for param_line in params_str.lines().take(10) {
            lines.push(Line::from(Span::styled(
                format!("  {}", param_line),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if params_str.lines().count() > 10 {
            lines.push(Line::from(Span::styled(
                "  ...",
                Style::default().fg(Color::DarkGray),
            )));
        }

        // Approval prompt if pending
        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        // Result if completed
        if let Some(ref result) = self.result {
            lines.extend(render_result(result, 5));
        }

        // Denied message
        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn status(&self) -> Status {
        self.status
    }

    fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    fn set_result(&mut self, result: String) {
        self.result = Some(result);
    }

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }
}

/// Helper: render approval prompt
pub fn render_approval_prompt() -> Line<'static> {
    Line::from(vec![
        Span::styled("  [", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "y",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("]es  [", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "n",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("]o  [", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "a",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("]lways", Style::default().fg(Color::DarkGray)),
    ])
}

/// Helper: render result with line limit
pub fn render_result(result: &str, max_lines: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let preview_lines: Vec<&str> = result.lines().take(max_lines).collect();
    for line in &preview_lines {
        lines.push(Line::from(Span::styled(
            format!("  {}", line),
            Style::default().fg(Color::DarkGray),
        )));
    }
    if result.lines().count() > max_lines {
        lines.push(Line::from(Span::styled(
            "  ...",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

/// A turn in the conversation - one user or assistant response
pub struct Turn {
    pub id: TurnId,
    pub role: Role,
    pub status: Status,
    pub content: Vec<Box<dyn Block>>,
    pub timestamp: DateTime<Utc>,
}

impl Turn {
    pub fn new(id: TurnId, role: Role, content: Vec<Box<dyn Block>>) -> Self {
        Self {
            id,
            role,
            status: Status::Success,
            content,
            timestamp: Utc::now(),
        }
    }

    /// Add a new text block and return its index for streaming
    pub fn add_text_block(&mut self, text: &str) -> usize {
        let idx = self.content.len();
        self.content.push(Box::new(TextBlock::new(text)));
        idx
    }

    /// Append text to a specific block by index
    pub fn append_to_block(&mut self, idx: usize, text: &str) {
        if let Some(block) = self.content.get_mut(idx) {
            block.append_text(text);
        }
    }

    /// Add a block
    pub fn add_block(&mut self, block: Box<dyn Block>) {
        self.content.push(block);
    }

    /// Get a mutable block by call_id
    pub fn get_block_mut(&mut self, call_id: &str) -> Option<&mut (dyn Block + 'static)> {
        for block in &mut self.content {
            if block.call_id() == Some(call_id) {
                return Some(block.as_mut());
            }
        }
        None
    }

    /// Render all blocks with given width
    pub fn render(&self, width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        for block in &self.content {
            lines.extend(block.render(width));
        }
        lines
    }
}

/// The chat transcript - display log of all turns for UI rendering
#[derive(Default)]
pub struct Transcript {
    turns: Vec<Turn>,
    next_id: usize,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            turns: Vec::new(),
            next_id: 0,
        }
    }

    fn next_id(&mut self) -> TurnId {
        let id = TurnId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Add a new turn with a single block
    pub fn add(&mut self, role: Role, block: impl Block + 'static) -> TurnId {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![Box::new(block)]));
        id
    }

    /// Add a new turn with a boxed block
    pub fn add_boxed(&mut self, role: Role, block: Box<dyn Block>) -> TurnId {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![block]));
        id
    }

    /// Add an empty turn (for streaming)
    pub fn add_empty(&mut self, role: Role) -> TurnId {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![]));
        id
    }

    pub fn get_mut(&mut self, id: TurnId) -> Option<&mut Turn> {
        self.turns.iter_mut().find(|t| t.id == id)
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn clear(&mut self) {
        self.turns.clear();
        self.next_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_block_render() {
        let block = TextBlock::new("Hello\nWorld");
        let lines = block.render(80);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_tool_block_status() {
        let mut block = ToolBlock::new("call_1", "test", serde_json::json!({}));
        assert_eq!(block.status(), Status::Pending);

        block.set_status(Status::Running);
        assert_eq!(block.status(), Status::Running);

        block.set_status(Status::Success);
        block.set_result("done".to_string());
        assert_eq!(block.status(), Status::Success);
    }

    #[test]
    fn test_transcript_add_and_get() {
        let mut transcript = Transcript::new();

        let id1 = transcript.add(Role::User, TextBlock::new("Hello"));
        let id2 = transcript.add(Role::Assistant, TextBlock::new("Hi there!"));

        assert_eq!(transcript.turns().len(), 2);
        assert_eq!(transcript.get_mut(id1).unwrap().role, Role::User);
        assert_eq!(transcript.get_mut(id2).unwrap().role, Role::Assistant);
    }
    
    #[test]
    fn test_turn_streaming() {
        let mut turn = Turn::new(TurnId(0), Role::Assistant, vec![]);
        
        // Start streaming - add a text block and get its index
        let idx = turn.add_text_block("Hello");
        assert_eq!(idx, 0);
        
        // Append to that block
        turn.append_to_block(idx, " world");
        
        assert_eq!(turn.content.len(), 1);
    }
}
