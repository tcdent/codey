//! IDE integrations for external editors
//!
//! This module provides a common trait for IDE integrations and implementations
//! for specific editors like Neovim.
//!
//! # Architecture
//!
//! The [`Ide`] trait defines a bidirectional interface:
//! - **Output**: Show previews, close previews, reload buffers, navigate to files
//! - **Input**: Check for unsaved changes
//! - **Events**: Selection changes streamed from the IDE
//!
//! The app holds an `Option<Box<dyn Ide>>` and calls these methods at appropriate
//! points in the tool execution flow. Tools can define what previews they produce
//! and what actions they need after execution.

pub mod nvim;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

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
}

/// A text selection from the IDE
#[derive(Debug, Clone)]
pub struct Selection {
    pub path: String,
    pub content: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Events streamed from the IDE to the app
#[derive(Debug, Clone)]
pub enum IdeEvent {
    /// Selection changed (or cleared if None)
    SelectionChanged(Option<Selection>),
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
        }
    }

    /// Navigate to a file and optionally a specific position
    async fn navigate_to(
        &self,
        path: &str,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<()>;

    /// Check if a file has unsaved changes
    async fn has_unsaved_changes(&self, path: &str) -> Result<bool>;

    // === Events: IDE → App (streaming) ===

    /// Get a mutable reference to the event receiver for polling
    /// Returns None if the IDE doesn't support event streaming
    fn event_receiver(&mut self) -> Option<&mut mpsc::Receiver<IdeEvent>>;
}
