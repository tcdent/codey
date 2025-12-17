//! Shell command execution tool

use super::{Tool, ToolResult};
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_result, Block, BlockType, ToolBlock, Status};
use anyhow::Result;
use async_trait::async_trait;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;

/// Shell command block - shows the command cleanly
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl ShellBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
        }
    }

    /// Create from tool params JSON
    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value) -> Option<Self> {
        let _: ShellParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for ShellBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let command = self.params["command"].as_str().unwrap_or("");
        let working_dir = self.params.get("working_dir").and_then(|v| v.as_str());

        // Format: shell(command) or shell(command, in dir)
        let mut spans = vec![
            self.render_status(),
            Span::styled("shell", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(command, Style::default().fg(Color::White)),
        ];
        if let Some(dir) = working_dir {
            spans.push(Span::styled(format!(", in {}", dir), Style::default().fg(Color::DarkGray)));
        }
        spans.push(Span::styled(")", Style::default().fg(Color::DarkGray)));
        lines.push(Line::from(spans));

        // Approval prompt if pending
        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        // Output if completed
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
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
        Some(&self.tool_name)
    }

    fn params(&self) -> Option<&serde_json::Value> {
        Some(&self.params)
    }
}

/// Tool for executing shell commands
pub struct ShellTool {
    timeout_secs: u64,
}

impl ShellTool {
    pub fn new() -> Self {
        Self { timeout_secs: 120 }
    }
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct ShellParams {
    command: String,
    working_dir: Option<String>,
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn description(&self) -> &'static str {
        "Execute a bash command and return stdout/stderr. \
         Use for: ls, grep, git, cargo, npm, etc. \
         Prefer read_file over cat/head/tail. \
         Commands are executed with a timeout."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Working directory for the command (optional, defaults to current directory)"
                }
            },
            "required": ["command"]
        })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = ShellBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            // Fallback to generic if params don't parse
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let params: ShellParams = serde_json::from_value(params)?;

        // Build command
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(&params.command);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Set working directory if specified
        if let Some(ref working_dir) = params.working_dir {
            let path = std::path::Path::new(working_dir);
            if !path.exists() {
                return Ok(ToolResult::error(format!(
                    "Working directory does not exist: {}",
                    working_dir
                )));
            }
            if !path.is_dir() {
                return Ok(ToolResult::error(format!(
                    "Not a directory: {}",
                    working_dir
                )));
            }
            cmd.current_dir(working_dir);
        }

        // Execute with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            cmd.output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut result_text = String::new();

                if !stdout.is_empty() {
                    result_text.push_str(&stdout);
                }

                if !stderr.is_empty() {
                    if !result_text.is_empty() {
                        result_text.push_str("\n\n");
                    }
                    result_text.push_str("[stderr]\n");
                    result_text.push_str(&stderr);
                }

                if result_text.is_empty() {
                    result_text = "(no output)".to_string();
                }

                // Add exit code if non-zero
                if exit_code != 0 {
                    result_text.push_str(&format!("\n\n[exit code: {}]", exit_code));
                }

                // Truncate if too long
                const MAX_OUTPUT: usize = 50000;
                if result_text.len() > MAX_OUTPUT {
                    result_text = format!(
                        "{}\n\n[... output truncated ({} bytes total)]",
                        &result_text[..MAX_OUTPUT],
                        result_text.len()
                    );
                }

                if output.status.success() {
                    Ok(ToolResult::success(result_text))
                } else {
                    Ok(ToolResult::error(result_text))
                }
            }
            Ok(Err(e)) => Ok(ToolResult::error(format!("Failed to execute command: {}", e))),
            Err(_) => Ok(ToolResult::error(format!(
                "Command timed out after {} seconds",
                self.timeout_secs
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shell_echo() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({
                "command": "echo 'hello world'"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("hello world"));
    }

    #[tokio::test]
    async fn test_shell_with_working_dir() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({
                "command": "pwd",
                "working_dir": "/tmp"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("/tmp"));
    }

    #[tokio::test]
    async fn test_shell_error() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({
                "command": "exit 1"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("exit code: 1"));
    }

    #[tokio::test]
    async fn test_shell_stderr() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({
                "command": "echo 'error message' >&2"
            }))
            .await
            .unwrap();

        assert!(result.content.contains("[stderr]"));
        assert!(result.content.contains("error message"));
    }

    #[tokio::test]
    async fn test_shell_invalid_working_dir() {
        let tool = ShellTool::new();
        let result = tool
            .execute(json!({
                "command": "ls",
                "working_dir": "/nonexistent/directory"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("does not exist"));
    }
}
