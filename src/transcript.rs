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
    Complete,
    Error,
    Denied,
    Cancelled,
}

/// Trait for all blocks in a turn
#[typetag::serde(tag = "type")]
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

    /// Get the text content of this block (for restoring agent context)
    fn text_content(&self) -> Option<&str> {
        None
    }

    /// Get the tool name (for restoring agent context)
    fn tool_name(&self) -> Option<&str> {
        None
    }

    /// Get the tool params (for restoring agent context)
    fn params(&self) -> Option<&serde_json::Value> {
        None
    }

    /// Get the tool result (for restoring agent context)
    fn result(&self) -> Option<&str> {
        None
    }

    /// Get thinking signature for restoring agent context
    fn signature(&self) -> Option<&str> {
        None
    }

    /// Set the thinking signature (called after streaming completes)
    fn set_signature(&mut self, _signature: &str) {}

    /// Downcast to concrete type for type-specific operations
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
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
            status: Status::Complete,
        }
    }
}

#[typetag::serde]
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

    fn text_content(&self) -> Option<&str> {
        Some(&self.text)
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Thinking/reasoning content (extended thinking)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub text: String,
    /// Signature for verification (required by Anthropic API for tool use continuation)
    #[serde(default)]
    pub signature: String,
}

impl ThinkingBlock {
    pub fn new(text: impl Into<String>, signature: impl Into<String>) -> Self {
        Self { 
            text: text.into(),
            signature: signature.into(),
        }
    }
}

#[typetag::serde]
impl Block for ThinkingBlock {
    fn render(&self, width: u16) -> Vec<Line<'_>> {
        use ratatui::style::{Color, Style};

        // Render thinking with dimmed style
        let skin = ratskin::RatSkin::default();
        let text = ratskin::RatSkin::parse_text(&self.text);
        let mut lines = skin.parse(text, width);

        // Apply dim styling to all lines
        let dim_style = Style::default().fg(Color::DarkGray);
        for line in &mut lines {
            let spans: Vec<_> = line
                .spans
                .iter()
                .map(|s| ratatui::text::Span::styled(s.content.clone(), dim_style))
                .collect();
            *line = Line::from(spans);
        }
        lines
    }

    fn status(&self) -> Status {
        Status::Complete
    }

    fn set_status(&mut self, _status: Status) {
        // Thinking blocks don't have mutable status
    }

    fn append_text(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn text_content(&self) -> Option<&str> {
        Some(&self.text)
    }

    fn signature(&self) -> Option<&str> {
        Some(&self.signature)
    }

    fn set_signature(&mut self, signature: &str) {
        self.signature = signature.to_string();
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
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

#[typetag::serde]
impl Block for ToolBlock {
    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let (icon, color) = match self.status {
            Status::Pending => ("?", Color::Yellow),
            Status::Running => ("⚙", Color::Blue),
            Status::Complete => ("✓", Color::Green),
            Status::Error => ("✗", Color::Red),
            Status::Denied => ("⊘", Color::DarkGray),
            Status::Cancelled => ("⊘", Color::Yellow),
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

    fn tool_name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn params(&self) -> Option<&serde_json::Value> {
        Some(&self.params)
    }

    fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Compaction summary block - shown when context was compacted
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionBlock {
    pub summary: String,
    pub previous_transcript: Option<String>,
    pub status: Status,
    pub context_tokens: Option<u32>,
}

impl CompactionBlock {
    pub fn new(summary: impl Into<String>, previous_transcript: Option<String>) -> Self {
        Self {
            summary: summary.into(),
            previous_transcript,
            status: Status::Complete,
            context_tokens: None,
        }
    }

    /// Create a pending compaction block (before summary is available)
    pub fn pending(context_tokens: u32, previous_transcript: Option<String>) -> Self {
        Self {
            summary: String::new(),
            previous_transcript,
            status: Status::Pending,
            context_tokens: Some(context_tokens),
        }
    }
}

#[typetag::serde]
impl Block for CompactionBlock {
    fn render(&self, width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        match self.status {
            Status::Pending | Status::Running => {
                // Show in-progress indicator
                let tokens_info = self.context_tokens
                    .map(|t| format!(" ({} tokens)", t))
                    .unwrap_or_default();

                lines.push(Line::from(vec![
                    Span::styled("📋 ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        format!("Compacting context{}", tokens_info),
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
                // Header with icon
                lines.push(Line::from(vec![
                    Span::styled("📋 ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        "Context Compacted",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));

                // Previous transcript reference if available
                if let Some(ref prev) = self.previous_transcript {
                    lines.push(Line::from(Span::styled(
                        format!("  Previous transcript: {}", prev),
                        Style::default().fg(Color::DarkGray),
                    )));
                }

                lines.push(Line::from(""));

                // Render the summary using markdown
                let skin = ratskin::RatSkin::default();
                let text = ratskin::RatSkin::parse_text(&self.summary);
                let summary_lines = skin.parse(text, width);

                // Indent summary lines
                for line in summary_lines {
                    lines.push(line);
                }
            }
            _ => {
                // Cancelled, Error, Denied - show appropriate message
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

    fn status(&self) -> Status {
        self.status
    }

    fn set_status(&mut self, status: Status) {
        self.status = status;
    }

    fn append_text(&mut self, text: &str) {
        self.summary.push_str(text);
    }

    fn text_content(&self) -> Option<&str> {
        if self.summary.is_empty() {
            None
        } else {
            Some(&self.summary)
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
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
#[derive(Serialize, Deserialize)]
pub struct Turn {
    pub id: TurnId,
    pub role: Role,
    pub status: Status,
    pub content: Vec<Box<dyn Block>>,
    pub timestamp: DateTime<Utc>,
}

impl Turn {
    pub fn new(id: TurnId, role: Role, content: Vec<Box<dyn Block>>, status: Status) -> Self {
        Self {
            id,
            role,
            status,
            content,
            timestamp: Utc::now(),
        }
    }


    /// Add a block and return its index
    pub fn add_block(&mut self, block: Box<dyn Block>) -> usize {
        let idx = self.content.len();
        self.content.push(block);
        idx
    }

    /// Append text to a specific block by index
    pub fn append_to_block(&mut self, idx: usize, text: &str) {
        if let Some(block) = self.content.get_mut(idx) {
            block.append_text(text);
        }
    }

    /// Get a mutable block by index
    pub fn get_block_mut(&mut self, idx: usize) -> Option<&mut (dyn Block + 'static)> {
        self.content.get_mut(idx).map(|b| b.as_mut())
    }

    /// Render all blocks with given width
    pub fn render(&self, width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        for (i, block) in self.content.iter().enumerate() {
            lines.extend(block.render(width));
            // Add blank line between blocks (but not after last)
            if i < self.content.len() - 1 {
                lines.push(Line::from(""));
            }
        }
        lines
    }
}

/// The chat transcript - display log of all turns for UI rendering
#[derive(Default, Serialize, Deserialize)]
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
    pub fn add(&mut self, role: Role, block: impl Block + 'static, status: Status) -> TurnId {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![Box::new(block)], status));
        id
    }

    /// Add a new turn with a boxed block
    pub fn add_boxed(&mut self, role: Role, block: Box<dyn Block>, status: Status) -> TurnId {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![block], status));
        id
    }

    /// Add an empty turn (for streaming)
    pub fn add_empty(&mut self, role: Role, status: Status) -> TurnId {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![], status));
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

    /// Save transcript to a JSON file
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(file, self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    /// Load transcript from a JSON file
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        serde_json::from_reader(file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
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

        block.set_status(Status::Complete);
        block.set_result("done".to_string());
        assert_eq!(block.status(), Status::Complete);
    }

    #[test]
    fn test_transcript_add_and_get() {
        let mut transcript = Transcript::new();

        let id1 = transcript.add(Role::User, TextBlock::new("Hello"), Status::Complete);
        let id2 = transcript.add(Role::Assistant, TextBlock::new("Hi there!"), Status::Complete);

        assert_eq!(transcript.turns().len(), 2);
        assert_eq!(transcript.get_mut(id1).unwrap().role, Role::User);
        assert_eq!(transcript.get_mut(id2).unwrap().role, Role::Assistant);
    }
    
    #[test]
    fn test_turn_streaming() {
        let mut turn = Turn::new(TurnId(0), Role::Assistant, vec![], Status::Running);
        
        // Start streaming - add a text block and get its index
        let idx = turn.add_block(Box::new(TextBlock::new("Hello")));
        assert_eq!(idx, 0);
        
        // Append to that block
        turn.append_to_block(idx, " world");
        
        assert_eq!(turn.content.len(), 1);
    }

    #[test]
    fn test_transcript_save_load_roundtrip() {
        let mut transcript = Transcript::new();
        transcript.add(Role::User, TextBlock::new("Hello"), Status::Complete);
        transcript.add(Role::Assistant, TextBlock::new("Hi there!"), Status::Complete);
        transcript.add(Role::User, TextBlock::new("How are you?"), Status::Complete);

        // Save to temp file
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("codey_test_transcript.json");

        transcript.save(&path).expect("Failed to save transcript");

        // Load and verify
        let loaded = Transcript::load(&path).expect("Failed to load transcript");
        assert_eq!(loaded.turns().len(), 3);
        assert_eq!(loaded.turns()[0].role, Role::User);
        assert_eq!(loaded.turns()[1].role, Role::Assistant);
        assert_eq!(loaded.turns()[2].role, Role::User);

        // Verify text content is preserved
        let text = loaded.turns()[0].content[0].text_content();
        assert_eq!(text, Some("Hello"));

        // Clean up
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_transcript_save_load_with_tool_blocks() {
        let mut transcript = Transcript::new();
        transcript.add(Role::User, TextBlock::new("Run ls"), Status::Complete);
        
        // Add an assistant turn with a tool block
        let mut tool_block = ToolBlock::new("call_123", "shell", serde_json::json!({"command": "ls"}));
        tool_block.set_status(Status::Complete);
        tool_block.set_result("file1.txt\nfile2.txt".to_string());
        transcript.add_boxed(Role::Assistant, Box::new(tool_block), Status::Complete);

        // Save to temp file
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("codey_test_tool_transcript.json");

        transcript.save(&path).expect("Failed to save transcript");

        // Load and verify
        let loaded = Transcript::load(&path).expect("Failed to load transcript");
        assert_eq!(loaded.turns().len(), 2);
        
        // Verify tool block is preserved
        let tool_turn = &loaded.turns()[1];
        assert_eq!(tool_turn.role, Role::Assistant);
        let block = &tool_turn.content[0];
        assert_eq!(block.tool_name(), Some("shell"));
        assert_eq!(block.call_id(), Some("call_123"));
        assert!(block.params().is_some());
        assert_eq!(block.result(), Some("file1.txt\nfile2.txt"));

        // Clean up
        let _ = std::fs::remove_file(path);
    }
}
