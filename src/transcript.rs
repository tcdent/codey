//! Core types for chat transcript
//!
//! This module contains the types that represent the conversation transcript.
//! A Transcript contains Turns, and each Turn contains Blocks.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};

use crate::app::{CODEY_DIR, TRANSCRIPTS_DIR};
use crate::compaction::CompactionBlock;


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

/// Block type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Text,
    Thinking,
    Tool,
    Compaction,
}

/// Get the transcripts directory path, creating it if necessary
fn get_transcripts_dir() -> std::io::Result<PathBuf> {
    let dir = PathBuf::from(CODEY_DIR).join(TRANSCRIPTS_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(dir)
}

/// Find the latest transcript number by scanning the transcripts directory
fn find_latest_transcript_number(dir: &Path) -> Option<u32> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.ends_with(".json") {
                name.trim_end_matches(".json").parse::<u32>().ok()
            } else {
                None
            }
        })
        .max()
}

/// Get the path for a transcript with a given number
fn transcript_path(dir: &Path, number: u32) -> PathBuf {
    dir.join(format!("{:06}.json", number))
}


/// Trait for all blocks in a turn
#[typetag::serde(tag = "type")]
pub trait Block: Send + Sync {
    /// Get the kind of this block
    fn kind(&self) -> BlockType;

    /// Render this block to terminal lines with given width for wrapping
    fn render(&self, width: u16) -> Vec<Line<'_>>;

    /// Get the status of this block
    fn status(&self) -> Status;

    /// Set the status of this block
    fn set_status(&mut self, status: Status);

    /// Render status icon with appropriate color
    fn render_status(&self) -> Span<'static> {
        let (icon, color) = match self.status() {
            Status::Pending => ("? ", Color::Yellow),
            Status::Running => ("⚙ ", Color::Blue),
            Status::Complete => ("✓ ", Color::Green),
            Status::Error => ("✗ ", Color::Red),
            Status::Denied => ("⊘ ", Color::DarkGray),
            Status::Cancelled => ("⊘ ", Color::Yellow),
        };
        Span::styled(icon, Style::default().fg(color))
    }

    /// Append text content to this block (for streaming)
    fn append_text(&mut self, _text: &str) {}

    /// Get the text content of this block (for restoring agent context)
    fn text(&self) -> Option<&str> { None }

    /// Get the tool call ID (for restoring agent context)
    fn call_id(&self) -> Option<&str> { None }

    /// Get the tool name (for restoring agent context)
    fn tool_name(&self) -> Option<&str> { None }

    /// Get the tool params (for restoring agent context)
    fn params(&self) -> Option<&serde_json::Value> { None }
}

/// Macro to implement common Block trait methods for blocks with text and status fields
#[macro_export]
macro_rules! impl_base_block {
    ($block_type:expr) => {
        fn kind(&self) -> BlockType {
            $block_type
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

        fn text(&self) -> Option<&str> {
            Some(&self.text)
        }
    };
}

/// Simple text content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBlock {
    pub text: String,
    pub status: Status,
}

impl TextBlock {
    // TODO Delete `new` and force using a keyword to create
    pub fn new(text: impl Into<String>) -> Self {
        Self { 
            text: text.into(),
            status: Status::Running,
        }
    }

    pub fn pending(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status: Status::Pending,
        }
    }

    pub fn complete(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            status: Status::Complete,
        }
    }
}

#[typetag::serde]
impl Block for TextBlock {
    impl_base_block!(BlockType::Text);

    fn render(&self, width: u16) -> Vec<Line<'_>> {
        // Use ratskin for markdown rendering
        let skin = ratskin::RatSkin::default();
        let text = ratskin::RatSkin::parse_text(&self.text);
        skin.parse(text, width)
    }
}

/// Thinking/reasoning content (extended thinking)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub text: String,
    pub status: Status,
}

impl ThinkingBlock {
    pub fn new(text: impl Into<String>) -> Self {
        Self { 
            text: text.into(),
            status: Status::Running,
        }
    }
}

#[typetag::serde]
impl Block for ThinkingBlock {
    impl_base_block!(BlockType::Thinking);

    fn render(&self, width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let style = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC);

        let wrapped = textwrap::wrap(&self.text, width as usize);
        for line in wrapped {
            lines.push(Line::from(Span::styled(line, style)));
        }
        lines
    }
}

/// Generic tool content (fallback for tools without specialized display)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolBlock {
    pub call_id: String,
    pub name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl ToolBlock {
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
        }
    }
}

#[typetag::serde]
impl Block for ToolBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        // Tool name with status icon
        lines.push(Line::from(vec![
            self.render_status(),
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
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
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

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }

    fn tool_name(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn params(&self) -> Option<&serde_json::Value> {
        Some(&self.params)
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
        Span::styled("]o", Style::default().fg(Color::DarkGray)),
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
    pub id: usize,
    pub role: Role,
    pub content: Vec<Box<dyn Block>>,
    pub timestamp: DateTime<Utc>,
    /// Index of the currently active (streaming) block, if any
    #[serde(skip)]
    pub active_block_idx: Option<usize>,
}

impl Turn {
    pub fn new(id: usize, role: Role, content: Vec<Box<dyn Block>>) -> Self {
        Self {
            id,
            role,
            content,
            timestamp: Utc::now(),
            active_block_idx: None,
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

    /// Mark a block as complete by index
    pub fn complete_block(&mut self, idx: usize) {
        if let Some(block) = self.get_block_mut(idx) {
            block.set_status(Status::Complete);
        }
    }

    /// Check if the active block matches the expected type
    pub fn is_active_block_type(&self, expected: BlockType) -> bool {
        self.active_block_idx
            .and_then(|idx| self.content.get(idx))
            .map(|block| block.kind() == expected)
            .unwrap_or(false)
    }

    /// Start a new block (completes previous active block if any)
    pub fn start_block(&mut self, block: Box<dyn Block>) -> usize {
        if let Some(prev_idx) = self.active_block_idx {
            self.complete_block(prev_idx);
        }
        let idx = self.add_block(block);
        self.active_block_idx = Some(idx);
        idx
    }

    /// Append text to the currently active block
    pub fn append_to_active(&mut self, text: &str) {
        if let Some(idx) = self.active_block_idx {
            self.append_to_block(idx, text);
        }
    }

    /// Get a mutable reference to the active block
    pub fn get_active_block_mut(&mut self) -> Option<&mut (dyn Block + 'static)> {
        self.active_block_idx.and_then(|idx| self.get_block_mut(idx))
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
#[derive(Serialize, Deserialize)]
pub struct Transcript {
    turns: Vec<Turn>,
    next_id: usize,
    #[serde(skip)]
    path: Option<PathBuf>,
    /// ID of the current turn being streamed to (if any)
    #[serde(skip)]
    current_turn_id: Option<usize>,
}

impl Transcript {
    /// Create a new transcript with a specific path
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            turns: Vec::new(),
            next_id: 0,
            path: Some(path),
            current_turn_id: None,
        }
    }

    /// Get the current path
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn next_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Add a new turn with a single block
    pub fn add_turn(&mut self, role: Role, block: impl Block + 'static) -> usize {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![Box::new(block)]));
        id
    }

    /// Add an empty turn (for streaming)
    pub fn add_empty(&mut self, role: Role) -> usize {
        let id = self.next_id();
        self.turns.push(Turn::new(id, role, vec![]));
        id
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Turn> {
        self.turns.iter_mut().find(|t| t.id == id)
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    // =========================================================================
    // Turn streaming lifecycle
    // =========================================================================

    /// Begin a new turn for streaming. Must call finish_turn() when done.
    pub fn begin_turn(&mut self, role: Role) {
        if self.current_turn_id.is_some() {
            panic!("Cannot begin turn: previous turn not finished");
        }
        let id = self.add_empty(role);
        self.current_turn_id = Some(id);
    }

    /// Finish the current turn - marks active block complete, clears current turn.
    pub fn finish_turn(&mut self) {
        self.mark_active_block(Status::Complete);
        self.current_turn_id = None;
    }

    /// Get mutable reference to the current turn. Panics if no turn is active.
    fn current_turn_mut(&mut self) -> &mut Turn {
        let turn_id = self.current_turn_id
            .expect("No active turn - call begin_turn() first");
        self.get_mut(turn_id)
            .expect("Current turn ID is invalid")
    }

    /// Stream a delta to the current turn.
    /// Appends to active block if type matches, otherwise starts a new block.
    /// Panics if no turn is active.
    pub fn stream_delta(&mut self, kind: BlockType, text: &str) {
        let turn = self.current_turn_mut();
        if turn.is_active_block_type(kind) {
            turn.append_to_active(text);
        } else {
            let block: Box<dyn Block> = match kind {
                BlockType::Text => Box::new(TextBlock::new(text)),
                BlockType::Thinking => Box::new(ThinkingBlock::new(text)),
                BlockType::Compaction => Box::new(CompactionBlock::new(text)),
                BlockType::Tool => panic!("Use start_block for tools"),
            };
            turn.start_block(block);
        }
    }

    /// Start a new block on the current turn. Panics if no turn is active.
    pub fn start_block(&mut self, block: Box<dyn Block>) {
        self.current_turn_mut().start_block(block);
    }

    /// Get mutable reference to the active block.
    pub fn active_block_mut(&mut self) -> Option<&mut (dyn Block + 'static)> {
        let turn_id = self.current_turn_id?;
        self.get_mut(turn_id)?.get_active_block_mut()
    }

    /// Find a tool block by its call_id.
    pub fn find_tool_block_mut(&mut self, call_id: &str) -> Option<&mut (dyn Block + 'static)> {
        for turn in &mut self.turns {
            for block in &mut turn.content {
                if block.call_id() == Some(call_id) {
                    return Some(block.as_mut());
                }
            }
        }
        None
    }

    /// Set status on the active block.
    pub fn mark_active_block(&mut self, status: Status) {
        if let Some(block) = self.active_block_mut() {
            block.set_status(status);
        }
    }

    /// Check if current turn's active block matches a type.
    pub fn is_streaming_block_type(&self, kind: BlockType) -> bool {
        self.current_turn_id
            .and_then(|id| self.turns.iter().find(|t| t.id == id))
            .map(|t| t.is_active_block_type(kind))
            .unwrap_or(false)
    }

    // =========================================================================
    // Persistence
    // =========================================================================

    /// Save transcript to its path
    pub fn save(&self) -> std::io::Result<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::Other, "No path set for transcript")
        })?;
        
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(file, self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    /// Load the latest transcript from the transcripts directory
    /// If no transcripts exist, creates a new one with number 0
    pub fn load() -> std::io::Result<Self> {
        let dir = get_transcripts_dir()?;
        
        if let Some(latest_number) = find_latest_transcript_number(&dir) {
            let path = transcript_path(&dir, latest_number);
            let file = std::fs::File::open(&path)?;
            let mut transcript: Self = serde_json::from_reader(file)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            transcript.path = Some(path);
            Ok(transcript)
        } else {
            // No transcripts exist, create a new one with number 0
            let path = transcript_path(&dir, 0);
            Ok(Self::with_path(path))
        }
    }

    /// Create a new empty transcript with the next available number
    pub fn new_numbered() -> std::io::Result<Self> {
        let dir = get_transcripts_dir()?;
        
        let next_number = find_latest_transcript_number(&dir)
            .map(|n| n + 1)
            .unwrap_or(0);
        
        let path = transcript_path(&dir, next_number);
        Ok(Self::with_path(path))
    }

    /// Rotate to a new transcript file
    /// Saves the current transcript and returns a new one with the next numbered path
    /// If the last turn contains a CompactionBlock, it will be added to the new transcript
    pub fn rotate(&self) -> std::io::Result<Self> {
        // Save current transcript
        self.save()?;

        // Get transcripts directory
        let dir = get_transcripts_dir()?;
        
        // Find next number
        let next_number = find_latest_transcript_number(&dir)
            .map(|n| n + 1)
            .unwrap_or(0);
        
        // Create new transcript with next path
        let new_path = transcript_path(&dir, next_number);
        let mut new_transcript = Self::with_path(new_path);
        
        // Check if last turn has a CompactionBlock and carry it over
        if let Some(last_turn) = self.turns.last() {
            for block in &last_turn.content {
                if block.kind() == BlockType::Compaction {
                    if let Some(summary_text) = block.text() {
                        use crate::compaction::CompactionBlock;
                        let mut compaction_block = CompactionBlock::new(summary_text.to_string());
                        compaction_block.status = Status::Complete;
                        new_transcript.add_turn(Role::Assistant, compaction_block);
                        break; // Only add the first compaction block found
                    }
                }
            }
        }
        
        Ok(new_transcript)
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
        block.append_text("done");
        assert_eq!(block.status(), Status::Complete);
    }

    #[test]
    fn test_transcript_add_and_get() {
        let mut transcript = Transcript::with_path(std::path::PathBuf::from("/tmp/test.md"));

        let id1 = transcript.add_turn(Role::User, TextBlock::new("Hello"));
        let id2 = transcript.add_turn(Role::Assistant, TextBlock::new("Hi there!"));

        assert_eq!(transcript.turns().len(), 2);
        assert_eq!(transcript.get_mut(id1).unwrap().role, Role::User);
        assert_eq!(transcript.get_mut(id2).unwrap().role, Role::Assistant);
    }
    
    #[test]
    fn test_turn_streaming() {
        let mut turn = Turn::new(0, Role::Assistant, vec![]);
        
        // Start streaming - add a text block and get its index
        let idx = turn.add_block(Box::new(TextBlock::new("Hello")));
        assert_eq!(idx, 0);
        
        // Append to that block
        turn.append_to_block(idx, " world");
        
        assert_eq!(turn.content.len(), 1);
    }

    #[test]
    fn test_transcript_save_load_roundtrip() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("codey_test_transcript.json");

        let mut transcript = Transcript::with_path(path.clone());
        transcript.add_turn(Role::User, TextBlock::new("Hello"));
        transcript.add_turn(Role::Assistant, TextBlock::new("Hi there!"));
        transcript.add_turn(Role::User, TextBlock::new("How are you?"));

        transcript.save().expect("Failed to save transcript");

        // Load by reading the file directly
        let file = std::fs::File::open(&path).expect("Failed to open file");
        let loaded: Transcript = serde_json::from_reader(file).expect("Failed to deserialize");
        
        assert_eq!(loaded.turns().len(), 3);
        assert_eq!(loaded.turns()[0].role, Role::User);
        assert_eq!(loaded.turns()[1].role, Role::Assistant);
        assert_eq!(loaded.turns()[2].role, Role::User);

        // Verify text content is preserved
        let text = loaded.turns()[0].content[0].text();
        assert_eq!(text, Some("Hello"));

        // Clean up
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_transcript_save_load_with_tool_blocks() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("codey_test_tool_transcript.json");

        let mut transcript = Transcript::with_path(path.clone());
        transcript.add_turn(Role::User, TextBlock::new("Run ls"));
        
        // Add an assistant turn with a tool block
        let mut tool_block = ToolBlock::new("call_123", "shell", serde_json::json!({"command": "ls"}));
        tool_block.set_status(Status::Complete);
        tool_block.append_text("file1.txt\nfile2.txt");
        transcript.add_turn(Role::Assistant, tool_block);

        transcript.save().expect("Failed to save transcript");

        // Load by reading the file directly
        let file = std::fs::File::open(&path).expect("Failed to open file");
        let loaded: Transcript = serde_json::from_reader(file).expect("Failed to deserialize");
        
        assert_eq!(loaded.turns().len(), 2);
        
        // Verify tool block is preserved
        let tool_turn = &loaded.turns()[1];
        assert_eq!(tool_turn.role, Role::Assistant);
        let block = &tool_turn.content[0];
        assert_eq!(block.tool_name(), Some("shell"));
        assert_eq!(block.call_id(), Some("call_123"));
        assert!(block.params().is_some());
        assert_eq!(block.text(), Some("file1.txt\nfile2.txt"));

        // Clean up
        let _ = std::fs::remove_file(path);
    }
}
