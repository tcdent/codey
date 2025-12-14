//! Shell command execution tool

use super::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;

/// Tool for executing shell commands
pub struct ShellTool {
    timeout_secs: u64,
}

impl ShellTool {
    pub fn new() -> Self {
        Self { timeout_secs: 120 }
    }

    pub fn with_timeout(timeout_secs: u64) -> Self {
        Self { timeout_secs }
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
