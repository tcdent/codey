//! Shell command execution tool

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_prefix, render_result, Block, BlockType, ToolBlock, Status};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Shell command block - shows the command cleanly
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl ShellBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }

    /// Create from tool params JSON
    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value, background: bool) -> Option<Self> {
        let _: ShellParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params, background))
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
            render_prefix(self.background),
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

impl ShellTool {
    pub const NAME: &'static str = "mcp_shell";
}

impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        Self::NAME
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
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id; use list_background_tasks/get_background_task to check status and retrieve results."
                }
            },
            "required": ["command"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: ShellParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        ToolPipeline::new()
            .await_approval()
            .then(handlers::Shell {
                command: parsed.command,
                working_dir: parsed.working_dir,
                timeout_secs: self.timeout_secs,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        if let Some(block) = ShellBlock::from_params(call_id, self.name(), params.clone(), background) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params, background))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolExecutor, ToolRegistry, ToolCall, ToolDecision};

    #[tokio::test]
    async fn test_shell_echo() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ShellTool::NAME.to_string(),
            params: json!({ "command": "echo 'hello world'" }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, .. }) = executor.next().await {
            assert!(content.contains("hello world"));
        } else {
            panic!("Expected Completed event");
        }
    }

    #[tokio::test]
    async fn test_shell_with_working_dir() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ShellTool::NAME.to_string(),
            params: json!({
                "command": "pwd",
                "working_dir": "/tmp"
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, .. }) = executor.next().await {
            assert!(content.contains("/tmp"));
        } else {
            panic!("Expected Completed event");
        }
    }

    #[tokio::test]
    async fn test_shell_error() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ShellTool::NAME.to_string(),
            params: json!({ "command": "exit 1" }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, .. }) = executor.next().await {
            assert!(content.contains("exit code: 1"));
        } else {
            panic!("Expected Completed event");
        }
    }

    #[tokio::test]
    async fn test_shell_stderr() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ShellTool::NAME.to_string(),
            params: json!({ "command": "echo 'error message' >&2" }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Completed { content, .. }) = executor.next().await {
            assert!(content.contains("[stderr]"));
            assert!(content.contains("error message"));
        } else {
            panic!("Expected Completed event");
        }
    }

    #[tokio::test]
    async fn test_shell_invalid_working_dir() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(ShellTool::new()));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: ShellTool::NAME.to_string(),
            params: json!({
                "command": "ls",
                "working_dir": "/nonexistent/directory"
            }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Error { content, .. }) = executor.next().await {
            assert!(content.contains("does not exist"));
        } else {
            panic!("Expected Error event");
        }
    }
}
