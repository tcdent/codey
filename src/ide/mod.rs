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

#[cfg(feature = "cli")]
pub mod nvim;

use anyhow::Result;
use async_trait::async_trait;

#[cfg(feature = "cli")]
pub use nvim::Nvim;

// ============================================================================
// Core Types
// ============================================================================

/// An edit operation (old_string → new_string)
#[derive(Debug, Clone)]
pub struct Edit {
    pub old_string: String,
    pub new_string: String,
}

/// A preview to show in the IDE before tool execution
#[derive(Debug, Clone)]
pub enum ToolPreview {
    /// Show file content (for write_file, showing what will be created)
    File { path: String, content: String },
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

    /// Show a file preview in the IDE (for new file creation)
    async fn show_preview(&self, preview: &ToolPreview) -> Result<()>;

    /// Show a diff preview with edits (hunks with context, not full file)
    async fn show_diff_preview(&self, path: &str, edits: &[Edit]) -> Result<()>;

    /// Close any open preview windows/buffers
    async fn close_preview(&self) -> Result<()>;

    /// Reload a buffer that was modified externally
    async fn reload_buffer(&self, path: &str) -> Result<()>;

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

    /// Poll for the next event from the IDE
    /// Returns None if no event is available or IDE doesn't support events
    async fn next(&mut self) -> Option<IdeEvent>;
}
