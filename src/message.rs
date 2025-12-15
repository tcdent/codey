//! Core message types for chat history
//!
//! This module contains the serializable message types that represent
//! the chat history. All content blocks implement the ContentBlock trait.

use chrono::{DateTime, Utc};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};

/// Unique identifier for a message
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub usize);

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

/// Trait for all content blocks in a message
pub trait ContentBlock: Send + Sync {
    /// Render this block to terminal lines with given width for wrapping
    fn render(&self, width: u16) -> Vec<Line<'_>>;

    /// Get tool status if this block requires approval
    fn status(&self) -> Option<Status> {
        None
    }

    /// Get tool name if this is a tool block
    fn tool_name(&self) -> Option<&str> {
        None
    }

    /// Get tool call ID if this is a tool block
    fn call_id(&self) -> Option<&str> {
        None
    }

    /// Approve execution (for tools)
    fn approve(&mut self) {}

    /// Deny execution (for tools)
    fn deny(&mut self) {}

    /// Mark as complete with result (for tools)
    fn complete(&mut self, _result: String, _is_error: bool) {}

    /// Append text to this block (for streaming text blocks)
    fn append_text(&mut self, _text: &str) {}

    /// Check if this is a text block
    fn is_text_block(&self) -> bool {
        false
    }
}

/// Simple text content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBlock {
    pub text: String,
}

impl TextBlock {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

impl ContentBlock for TextBlock {
    fn render(&self, width: u16) -> Vec<Line<'_>> {
        // Use ratskin for markdown rendering
        let skin = ratskin::RatSkin::default();
        let text = ratskin::RatSkin::parse_text(&self.text);
        skin.parse(text, width)
    }

    fn append_text(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn is_text_block(&self) -> bool {
        true
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

impl ContentBlock for ToolBlock {
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

    fn status(&self) -> Option<Status> {
        Some(self.status)
    }

    fn tool_name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }

    fn approve(&mut self) {
        self.status = Status::Running;
    }

    fn deny(&mut self) {
        self.status = Status::Denied;
    }

    fn complete(&mut self, result: String, is_error: bool) {
        self.status = if is_error {
            Status::Error
        } else {
            Status::Success
        };
        self.result = Some(result);
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

/// A message in the chat history
pub struct Message {
    pub id: MessageId,
    pub role: Role,
    pub status: Status,
    pub content: Vec<Box<dyn ContentBlock>>,
    pub timestamp: DateTime<Utc>,
}

impl Message {
    pub fn new(id: MessageId, role: Role, content: Vec<Box<dyn ContentBlock>>) -> Self {
        Self {
            id,
            role,
            status: Status::Success, // Default to complete for most messages
            content,
            timestamp: Utc::now(),
        }
    }

    /// Append text to the last text block, or create a new one
    pub fn append_text(&mut self, text: &str) {
        // Try to append to the last block if it's a text block
        if let Some(block) = self.content.last_mut() {
            if block.is_text_block() {
                block.append_text(text);
                return;
            }
        }
        // No text block found, create one
        self.content.push(Box::new(TextBlock::new(text)));
    }

    /// Get a mutable tool block by call_id
    pub fn get_tool_mut(&mut self, call_id: &str) -> Option<&mut (dyn ContentBlock + 'static)> {
        for block in &mut self.content {
            if block.call_id() == Some(call_id) {
                return Some(block.as_mut());
            }
        }
        None
    }

    /// Render all content blocks with given width
    pub fn render(&self, width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        for block in &self.content {
            lines.extend(block.render(width));
        }
        lines
    }
}

/// The chat transcript - display log of all messages for UI rendering
#[derive(Default)]
pub struct Transcript {
    messages: Vec<Message>,
    next_id: usize,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            next_id: 0,
        }
    }

    fn next_id(&mut self) -> MessageId {
        let id = MessageId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn add(&mut self, role: Role, block: impl ContentBlock + 'static) -> MessageId {
        let id = self.next_id();
        self.messages.push(Message::new(id, role, vec![Box::new(block)]));
        id
    }

    pub fn add_boxed(&mut self, role: Role, block: Box<dyn ContentBlock>) -> MessageId {
        let id = self.next_id();
        self.messages.push(Message::new(id, role, vec![block]));
        id
    }

    pub fn get_mut(&mut self, id: MessageId) -> Option<&mut Message> {
        self.messages.iter_mut().find(|m| m.id == id)
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn clear(&mut self) {
        self.messages.clear();
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
        assert_eq!(block.status(), Some(Status::Pending));

        block.approve();
        assert_eq!(block.status(), Some(Status::Running));

        block.complete("done".to_string(), false);
        assert_eq!(block.status(), Some(Status::Success));
    }

    #[test]
    fn test_transcript_add_and_get() {
        let mut transcript = Transcript::new();

        let id1 = transcript.add(Role::User, TextBlock::new("Hello"));
        let id2 = transcript.add(Role::Assistant, TextBlock::new("Hi there!"));

        assert_eq!(transcript.messages().len(), 2);
        assert_eq!(transcript.get_mut(id1).unwrap().role, Role::User);
        assert_eq!(transcript.get_mut(id2).unwrap().role, Role::Assistant);
    }
    
    #[test]
    fn test_message_append_text() {
        let mut msg = Message::new(MessageId(0), Role::Assistant, vec![
            Box::new(TextBlock::new("Hello"))
        ]);
        
        msg.append_text(" world");
        
        // Should have appended to existing block, not created new one
        assert_eq!(msg.content.len(), 1);
    }
}
