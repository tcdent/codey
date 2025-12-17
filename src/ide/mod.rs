//! IDE integrations for external editors
//!
//! This module provides a common trait for IDE integrations and implementations
//! for specific editors like Neovim.
//!
//! # Architecture
//!
//! The [`Ide`] trait defines a bidirectional interface:
//! - **Output**: Show previews, close previews, reload buffers, navigate to files
//! - **Input**: Get selections, current file info, cursor position
//!
//! The app holds an `Option<Box<dyn Ide>>` and calls these methods at appropriate
//! points in the tool execution flow. Tools can define what previews they produce
//! and what actions they need after execution.

pub mod nvim;

use anyhow::Result;
use async_trait::async_trait;

pub use nvim::Nvim;

// ============================================================================
// Core Types
// ============================================================================

/// A preview to show in the IDE before tool execution
#[derive(Debug, Clone)]
pub enum ToolPreview {
    /// Show a side-by-side diff (for file edits)
    Diff {
        path: String,
        original: String,
        modified: String,
    },
    /// Show file content (for write_file, showing what will be created)
    FileContent {
        path: String,
        content: String,
    },
    // Future: CommandOutput, DirectoryListing, etc.
}

/// An action for the IDE to perform after tool execution
#[derive(Debug, Clone)]
pub enum IdeAction {
    /// Reload a buffer that was modified externally
    ReloadBuffer(String),
    /// Navigate to a specific location
    NavigateTo {
        path: String,
        line: Option<u32>,
        column: Option<u32>,
    },
    /// Show a notification message
    Notify {
        message: String,
        level: NotifyLevel,
    },
}

/// Notification severity level
#[derive(Debug, Clone, Copy, Default)]
pub enum NotifyLevel {
    #[default]
    Info,
    Warn,
    Error,
}

/// A text selection from the IDE
#[derive(Debug, Clone)]
pub struct Selection {
    pub path: String,
    pub content: String,
    pub start_line: u32,
    pub end_line: u32,
    pub start_col: Option<u32>,
    pub end_col: Option<u32>,
}

/// Information about the currently focused file
#[derive(Debug, Clone)]
pub struct CurrentFile {
    pub path: String,
    pub cursor_line: u32,
    pub cursor_col: u32,
    pub total_lines: u32,
}

/// Information about the visible range in the editor
#[derive(Debug, Clone)]
pub struct VisibleRange {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

// ============================================================================
// IDE Trait
// ============================================================================

/// Common interface for IDE integrations
///
/// This trait abstracts over different editors (Neovim, VSCode, etc.)
/// providing a unified interface for the app to interact with.
#[async_trait]
pub trait Ide: Send + Sync {
    /// Get a display name for this IDE (for logging/UI)
    fn name(&self) -> &'static str;

    // === Output: App → IDE ===

    /// Show a preview in the IDE (e.g., diff before approval)
    async fn show_preview(&self, preview: &ToolPreview) -> Result<()>;

    /// Close any open preview windows/buffers
    async fn close_preview(&self) -> Result<()>;

    /// Reload a buffer that was modified externally
    async fn reload_buffer(&self, path: &str) -> Result<()>;

    /// Execute an IDE action
    async fn execute(&self, action: &IdeAction) -> Result<()> {
        match action {
            IdeAction::ReloadBuffer(path) => self.reload_buffer(path).await,
            IdeAction::NavigateTo { path, line, column } => {
                self.navigate_to(path, *line, *column).await
            }
            IdeAction::Notify { message, level } => self.notify(message, *level).await,
        }
    }

    /// Navigate to a file and optionally a specific position
    async fn navigate_to(
        &self,
        path: &str,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<()>;

    /// Show a notification in the IDE
    async fn notify(&self, message: &str, level: NotifyLevel) -> Result<()>;

    // === Input: IDE → App ===

    /// Get the current text selection (if any)
    async fn get_selection(&self) -> Result<Option<Selection>>;

    /// Get information about the currently focused file
    async fn get_current_file(&self) -> Result<Option<CurrentFile>>;

    /// Get the visible line range in the current window
    async fn get_visible_range(&self) -> Result<Option<VisibleRange>>;

    /// Check if a file has unsaved changes
    async fn has_unsaved_changes(&self, path: &str) -> Result<bool>;
}
