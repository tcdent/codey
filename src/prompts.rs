//! Centralized prompt definitions and system prompt management.
//!
//! This module contains all system prompts used throughout the application,
//! as well as the `SystemPrompt` struct for building dynamic prompts.

use std::path::{Path, PathBuf};

use mdsh::Processor;

use crate::config::{Config, CODEY_DIR};

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

### Background Execution
For long-running operations, you can execute tools in the background by adding `"background": true` to the tool call. This returns immediately with a task ID while the tool runs asynchronously.
Use `list_background_tasks` to check status and `get_background_task` to retrieve results when complete.

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

/// A system prompt builder that supports dynamic content via mdsh.
///
/// The prompt is composed of:
/// 1. The base system prompt (static)
/// 2. User SYSTEM.md from ~/.config/codey/ (optional, dynamic)
/// 3. Project SYSTEM.md from .codey/ (optional, dynamic)
///
/// SYSTEM.md files are processed through [mdsh](https://github.com/zimbatm/mdsh),
/// allowing embedded shell commands to be executed and their output included.
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
    /// shell commands via mdsh. The result is the concatenation of:
    /// - Base system prompt
    /// - User SYSTEM.md content (if exists)
    /// - Project SYSTEM.md content (if exists)
    pub fn build(&self) -> String {
        let mut prompt = SYSTEM_PROMPT.to_string();

        // Append user SYSTEM.md if it exists
        if let Some(ref user_path) = self.user_path {
            if let Ok(content) = std::fs::read_to_string(user_path) {
                tracing::debug!("Appending user SYSTEM.md from {:?}", user_path);
                let processed = self.process_mdsh(&content, user_path);
                prompt.push_str("\n\n");
                prompt.push_str(&processed);
            }
        }

        // Append project SYSTEM.md if it exists
        if let Ok(content) = std::fs::read_to_string(&self.project_path) {
            tracing::debug!("Appending project SYSTEM.md from {:?}", self.project_path);
            let processed = self.process_mdsh(&content, &self.project_path);
            prompt.push_str("\n\n");
            prompt.push_str(&processed);
        }

        prompt
    }

    /// Process content through mdsh, executing embedded shell commands.
    fn process_mdsh(&self, content: &str, path: &Path) -> String {
        let workdir = path
            .parent()
            .map(|p| p.as_os_str())
            .unwrap_or_else(|| std::ffi::OsStr::new("."));

        let mut output = Vec::new();
        let mut processor = mdsh::executor::TheProcessor::new(workdir, &mut output);

        if let Err(e) = processor.process(content, &mdsh::cli::FileArg::StdHandle) {
            tracing::warn!("Failed to process mdsh content from {:?}: {}", path, e);
            return content.to_string();
        }

        String::from_utf8(output).unwrap_or_else(|_| content.to_string())
    }
}

impl Default for SystemPrompt {
    fn default() -> Self {
        Self::new()
    }
}
