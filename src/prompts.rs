//! Centralized prompt definitions and system prompt management.
//!
//! This module contains all system prompts used throughout the application,
//! as well as the `SystemPrompt` struct for building dynamic prompts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{Config, CODEY_DIR};

/// Embedded esh script for template processing
const ESH_SCRIPT: &str = include_str!("../lib/esh/esh");

/// Filename for custom system prompt additions
pub const SYSTEM_MD_FILENAME: &str = "SYSTEM.md";

/// Welcome message shown when the application starts
pub const WELCOME_MESSAGE: &str =
    "Welcome to Codey! I'm your AI coding assistant. How can I help you today?";

/// Main system prompt for the primary agent
pub const SYSTEM_PROMPT: &str = r#"You are Codey, an AI coding assistant running in a terminal interface.

## Capabilities
You have access to the following tools:
- `read_file`: Read file contents, optionally with line ranges
- `write_file`: Create new files
- `edit_file`: Make precise edits using search/replace
- `shell`: Execute bash commands
- `fetch_url`: Fetch web content

## Guidelines

### Reading Files
- Always read a file before editing it
- Use line ranges for large files: `read_file(path, start_line=100, end_line=200)`
- Use `shell("ls -la")` to explore directories
- When reading files, be careful about reading large files in one-go. Use line ranges,
    or check the file stats with `shell("stat <file_path>")` first.
- shell grep is a great way to get a line number to read a targeted section of a file

### Editing Files
- Use `edit_file` for existing files, `write_file` only for new files
- The `old_string` must match EXACTLY, including whitespace and indentation
- If `old_string` appears multiple times, include more context to make it unique
- Apply edits sequentially; each edit sees the result of previous edits
- You can do multiple edits at once, but keep it under 1000 lines
- Avoid the urge to completely rewrite files - make precise, minimal edits so the user can review them easily

### Shell Commands
- Prefer `read_file` over `cat`, `head`, `tail`
- Use `ls` for directory exploration
- Use `grep` or `rg` for searching code
- Prefer relative paths over absolute paths when possible - tool approval configs often allow execution from the current working directory but restrict access to system-wide paths
- Avoid using `cd` to change directories before commands; instead, use relative paths from your working directory (e.g., `./src/main.rs` or `src/main.rs`)

### Web Content
When fetching web pages with `fetch_html`, consider using `spawn_agent` to delegate content extraction to a sub-agent. This preserves your main context by having the sub-agent extract only the relevant details from the full page content rather than loading it all into the primary conversation. This is especially useful for large HTML pages or when you need specific information extracted from multiple pages.

### Background Execution
For long-running operations, you can execute tools in the background by adding `"background": true` to the tool call. This returns immediately with a task ID while the tool runs asynchronously.
Use `list_background_tasks` to check status and `get_background_task` to retrieve results when complete.

When running background agents, use `shell("sleep N")` (where N is seconds) to keep your execution loop active. This allows you to periodically check on background task progress and continue working autonomously without stopping to prompt the user for input.

### General
- Be concise but thorough
- Explain what you're doing before executing tools
- If a tool fails, explain the error and suggest fixes
- Ask for clarification if the request is ambiguous
- If you feel like backing out of a path, always get confirmation before git resetting the work tree
- Always get confirmation before making destructive changes (this includes building a release)
"#;

/// Prompt used when compacting conversation context
pub const COMPACTION_PROMPT: &str = r#"The conversation context is getting large and needs to be compacted.

Please provide a comprehensive summary of our conversation so far in markdown format. Include:

1. **What was accomplished** - Main tasks and changes completed as a bulleted list
2. **What still needs to be done** - Remaining tasks or open areas of work as a bulleted list
3. **Key project information** - Important facts about the project that the user has shared or that we're not immediately apparent
4. **Relevant files** - Files most relevant to the current work with brief descriptions, line numbers, or method/variable names
5. **Relevant documentation paths or URLs** - Links to docs or resources we will use to continue our work
6. **Quotes and log snippets** - Any important quotes or logs that the user provided that we'll need later

Be thorough but concise - this summary will seed a fresh conversation context."#;

/// System prompt for sub-agents (background research agents)
pub const SUB_AGENT_PROMPT: &str = r#"You are a background research agent. Your task is to investigate, explore, or analyze as directed.

## Capabilities
You have read-only access to:
- `read_file`: Read file contents
- `shell`: Execute commands (for searching, exploring)
- `fetch_url`: Fetch web content
- `web_search`: Search the web
- `open_file`: Signal a file to open in the IDE

## Guidelines
- Focus on the specific task assigned to you
- Be thorough but concise in your findings
- Report back with structured, actionable information
- You cannot modify files - only read and explore
- If you need to suggest changes, describe them clearly for the primary agent to implement
"#;

/// A system prompt builder that supports dynamic content via esh templates.
///
/// The prompt is composed of:
/// 1. The base system prompt (static)
/// 2. User SYSTEM.md from ~/.config/codey/ (optional, dynamic)
/// 3. Project SYSTEM.md from .codey/ (optional, dynamic)
///
/// SYSTEM.md files are processed through [esh](https://github.com/jirutka/esh),
/// allowing embedded shell commands using `<%= command %>` syntax.
#[derive(Clone)]
pub struct SystemPrompt {
    user_path: Option<PathBuf>,
    project_path: PathBuf,
}

impl SystemPrompt {
    /// Create a new SystemPrompt with default paths.
    pub fn new() -> Self {
        let user_path = Config::config_dir().map(|d| d.join(SYSTEM_MD_FILENAME));
        let project_path = Path::new(CODEY_DIR).join(SYSTEM_MD_FILENAME);

        Self {
            user_path,
            project_path,
        }
    }

    /// Build the complete system prompt.
    ///
    /// This reads and processes all SYSTEM.md files, executing any embedded
    /// shell commands via esh (`<%= command %>`). The result is the concatenation of:
    /// - Base system prompt
    /// - User SYSTEM.md content (if exists)
    /// - Project SYSTEM.md content (if exists)
    pub fn build(&self) -> String {
        let mut prompt = SYSTEM_PROMPT.to_string();

        // Append user SYSTEM.md if it exists
        if let Some(ref user_path) = self.user_path {
            if let Some(content) = self.load_system_md(user_path) {
                prompt.push_str("\n\n");
                prompt.push_str(&content);
            }
        }

        // Append project SYSTEM.md if it exists
        if let Some(content) = self.load_system_md(&self.project_path) {
            prompt.push_str("\n\n");
            prompt.push_str(&content);
        }

        prompt
    }

    /// Load and process a SYSTEM.md file through esh, falling back to raw content.
    fn load_system_md(&self, path: &Path) -> Option<String> {
        if !path.exists() {
            return None;
        }
        self.process_esh(path)
            .or_else(|| fs::read_to_string(path).ok())
            .filter(|s| !s.is_empty())
    }

    /// Ensure the esh script is available in the cache directory.
    /// Returns the path to the esh executable.
    // TODO: Use system esh if installed (e.g., `which esh`) before falling back to vendored version.
    fn ensure_esh() -> Option<PathBuf> {
        let cache_dir = dirs::cache_dir()?.join("codey");
        let esh_path = cache_dir.join("esh");

        if !esh_path.exists() {
            fs::create_dir_all(&cache_dir).ok()?;
            fs::write(&esh_path, ESH_SCRIPT).ok()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&esh_path, fs::Permissions::from_mode(0o755)).ok()?;
            }
        }

        Some(esh_path)
    }

    /// Process content through esh, executing embedded shell commands.
    /// Uses `<%= $(command) %>` syntax for command substitution.
    fn process_esh(&self, path: &Path) -> Option<String> {
        let esh_path = Self::ensure_esh()?;
        let workdir = path.parent().unwrap_or(Path::new("."));
        let filename = path.file_name()?;

        let output = Command::new(&esh_path)
            .arg(filename)
            .current_dir(workdir)
            .output()
            .ok()?;

        if output.status.success() {
            String::from_utf8(output.stdout).ok()
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("esh failed for {:?}: {}", path, stderr);
            None
        }
    }
}

impl Default for SystemPrompt {
    fn default() -> Self {
        Self::new()
    }
}
