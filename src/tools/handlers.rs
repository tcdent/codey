//! Effect handlers
//!
//! Each handler is a struct that knows how to execute a specific effect.
//! Handlers are stateless - they receive data and produce a Step result.

use crate::ide::{Edit, ToolPreview};
use crate::tools::io;
use crate::tools::pipeline::{Effect, EffectHandler, Step};
use std::fs;
use std::path::PathBuf;

// =============================================================================
// Validation handlers
// =============================================================================

/// Validate file exists, is a regular file, and is readable
pub struct ValidateFile {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl EffectHandler for ValidateFile {
    async fn call(self: Box<Self>) -> Step {
        match fs::metadata(&self.path) {
            Ok(m) if m.is_file() => Step::Continue,
            Ok(_) => Step::Error(format!("Not a file: {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Step::Error(format!("File not found: {}", self.path.display()))
            }
            Err(e) => Step::Error(format!("Cannot access {}: {}", self.path.display(), e)),
        }
    }
}

/// Validate file is writable (checks permissions)
pub struct ValidateFileWritable {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl EffectHandler for ValidateFileWritable {
    async fn call(self: Box<Self>) -> Step {
        // Check if we can open the file for writing
        match fs::OpenOptions::new().write(true).open(&self.path) {
            Ok(_) => Step::Continue,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Step::Error(format!("Permission denied: {}", self.path.display()))
            }
            Err(e) => Step::Error(format!("Cannot write to {}: {}", self.path.display(), e)),
        }
    }
}

/// Validate IDE buffer has no unsaved changes
pub struct ValidateNoUnsavedEdits {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl EffectHandler for ValidateNoUnsavedEdits {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::IdeCheckUnsavedEdits { path: self.path })
    }
}

/// Validate file does NOT exist (for write_file)
pub struct ValidateFileNotExists {
    pub path: PathBuf,
    pub message: String,
}

#[async_trait::async_trait]
impl EffectHandler for ValidateFileNotExists {
    async fn call(self: Box<Self>) -> Step {
        if self.path.exists() {
            Step::Error(self.message)
        } else {
            Step::Continue
        }
    }
}

/// Apply edits to a file. Assumes validation already passed.
pub struct ApplyEdits {
    pub path: PathBuf,
    pub edits: Vec<Edit>,
}

#[async_trait::async_trait]
impl EffectHandler for ApplyEdits {
    async fn call(self: Box<Self>) -> Step {
        let mut content = match fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) => return Step::Error(format!("Failed to read file: {}", e)),
        };

        for edit in &self.edits {
            debug_assert_eq!(
                content.matches(&edit.old_string).count(),
                1,
                "edit validation should have caught this"
            );
            content = content.replacen(&edit.old_string, &edit.new_string, 1);
        }

        match fs::write(&self.path, content) {
            Ok(()) => Step::Continue,
            Err(e) => Step::Error(format!("Failed to write file: {}", e)),
        }
    }
}
// =============================================================================
// Control flow handlers
// =============================================================================

/// Approval checkpoint - pauses pipeline until user approves
pub struct AwaitApproval;

#[async_trait::async_trait]
impl EffectHandler for AwaitApproval {
    async fn call(self: Box<Self>) -> Step {
        Step::AwaitApproval
    }
}

/// Set pipeline output
pub struct Output {
    pub content: String,
}

#[async_trait::async_trait]
impl EffectHandler for Output {
    async fn call(self: Box<Self>) -> Step {
        Step::Output(self.content)
    }
}

/// Emit streaming delta
pub struct Delta {
    pub content: String,
}

#[async_trait::async_trait]
impl EffectHandler for Delta {
    async fn call(self: Box<Self>) -> Step {
        Step::Delta(self.content)
    }
}

/// Fail the pipeline
pub struct Error {
    pub message: String,
}

#[async_trait::async_trait]
impl EffectHandler for Error {
    async fn call(self: Box<Self>) -> Step {
        Step::Error(self.message)
    }
}

// =============================================================================
// File system handlers
// =============================================================================

/// Read a file with line numbers
pub struct ReadFile {
    pub path: PathBuf,
    pub start_line: Option<i32>,
    pub end_line: Option<i32>,
}

#[async_trait::async_trait]
impl EffectHandler for ReadFile {
    async fn call(self: Box<Self>) -> Step {
        match io::read_file(&self.path, self.start_line, self.end_line) {
            Ok(content) => Step::Output(content),
            Err(e) => Step::Error(e),
        }
    }
}

/// Write content to a file
pub struct WriteFile {
    pub path: PathBuf,
    pub content: String,
}

#[async_trait::async_trait]
impl EffectHandler for WriteFile {
    async fn call(self: Box<Self>) -> Step {
        // Create parent directories if needed
        if let Some(parent) = self.path.parent() {
            if !parent.exists() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Step::Error(format!(
                        "Failed to create directory {}: {}",
                        parent.display(),
                        e
                    ));
                }
            }
        }
        match fs::write(&self.path, &self.content) {
            Ok(()) => Step::Continue,
            Err(e) => Step::Error(format!("Failed to write {}: {}", self.path.display(), e)),
        }
    }
}

// =============================================================================
// Shell handler
// =============================================================================

/// Execute a shell command
pub struct Shell {
    pub command: String,
    pub working_dir: Option<String>,
    pub timeout_secs: u64,
}

#[async_trait::async_trait]
impl EffectHandler for Shell {
    async fn call(self: Box<Self>) -> Step {
        match io::execute_shell(&self.command, self.working_dir.as_deref(), self.timeout_secs).await
        {
            Ok(result) if result.success => Step::Output(result.output),
            Ok(result) => Step::Output(result.output), // Still output, but includes exit code
            Err(e) => Step::Error(e),
        }
    }
}

// =============================================================================
// Network handlers
// =============================================================================

/// Fetch content from a URL
pub struct FetchUrl {
    pub url: String,
    pub max_length: Option<usize>,
}

#[async_trait::async_trait]
impl EffectHandler for FetchUrl {
    async fn call(self: Box<Self>) -> Step {
        match io::fetch_url(&self.url, self.max_length).await {
            Ok(result) => {
                let header = format!(
                    "[URL: {}]\n[Content-Type: {}]\n[Size: {} bytes]\n\n",
                    self.url, result.content_type, result.size
                );
                Step::Output(header + &result.content)
            }
            Err(e) => Step::Error(e),
        }
    }
}

/// Search the web
pub struct WebSearch {
    pub query: String,
    pub count: u32,
}

#[async_trait::async_trait]
impl EffectHandler for WebSearch {
    async fn call(self: Box<Self>) -> Step {
        match io::web_search(&self.query, self.count).await {
            Ok(results) => {
                if results.is_empty() {
                    Step::Output("No results found.".to_string())
                } else {
                    let output = results
                        .iter()
                        .enumerate()
                        .map(|(i, r)| format!("{}. [{}]({})", i + 1, r.title, r.url))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Step::Output(output)
                }
            }
            Err(e) => Step::Error(e),
        }
    }
}

// =============================================================================
// IDE handlers (delegate to app)
// =============================================================================

/// Open a file in the IDE
pub struct IdeOpen {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[async_trait::async_trait]
impl EffectHandler for IdeOpen {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::IdeOpen {
            path: self.path,
            line: self.line,
            column: self.column,
        })
    }
}

/// Show a file preview in the IDE (for new file creation)
pub struct IdeShowPreview {
    pub preview: ToolPreview,
}

#[async_trait::async_trait]
impl EffectHandler for IdeShowPreview {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::IdeShowPreview {
            preview: self.preview,
        })
    }
}

/// Show a diff preview in the IDE (edits with context, not full file)
pub struct IdeShowDiffPreview {
    pub path: PathBuf,
    pub edits: Vec<Edit>,
}

#[async_trait::async_trait]
impl EffectHandler for IdeShowDiffPreview {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::IdeShowDiffPreview {
            path: self.path,
            edits: self.edits,
        })
    }
}

/// Reload a buffer in the IDE
pub struct IdeReloadBuffer {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl EffectHandler for IdeReloadBuffer {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::IdeReloadBuffer { path: self.path })
    }
}

/// Close the IDE preview
pub struct IdeClosePreview;

#[async_trait::async_trait]
impl EffectHandler for IdeClosePreview {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::IdeClosePreview)
    }
}

// =============================================================================
// Agent handlers (delegate to app)
// =============================================================================

/// Spawn a background agent
pub struct SpawnAgent {
    pub task: String,
    pub context: Option<String>,
}

#[async_trait::async_trait]
impl EffectHandler for SpawnAgent {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::SpawnAgent {
            task: self.task,
            context: self.context,
        })
    }
}

/// Send a notification
pub struct Notify {
    pub message: String,
}

#[async_trait::async_trait]
impl EffectHandler for Notify {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::Notify {
            message: self.message,
        })
    }
}

// =============================================================================
// Background task handlers (delegate to app)
// =============================================================================

/// List all background tasks
pub struct ListBackgroundTasks;

#[async_trait::async_trait]
impl EffectHandler for ListBackgroundTasks {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::ListBackgroundTasks)
    }
}

/// Get a specific background task result
pub struct GetBackgroundTask {
    pub task_id: String,
}

#[async_trait::async_trait]
impl EffectHandler for GetBackgroundTask {
    async fn call(self: Box<Self>) -> Step {
        Step::Delegate(Effect::GetBackgroundTask {
            task_id: self.task_id,
        })
    }
}

// =============================================================================
// HTML content handlers
// =============================================================================

/// Fetch HTML content using headless browser and convert to readable markdown
pub struct FetchHtml {
    pub url: String,
    pub max_length: Option<usize>,
}

#[async_trait::async_trait]
impl EffectHandler for FetchHtml {
    async fn call(self: Box<Self>) -> Step {
        match io::fetch_html(&self.url, self.max_length).await {
            Ok(result) => {
                let title_info = result
                    .title
                    .as_ref()
                    .map(|t| format!("[Title: {}]\n", t))
                    .unwrap_or_default();
                let header = format!("[URL: {}]\n{}\n", result.url, title_info);
                Step::Output(header + &result.content)
            }
            Err(e) => Step::Error(e),
        }
    }
}
