//! Shell command execution tool

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{render_tool_block, Block, BlockType, ToolBlock, Status};
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
        let command = self.params["command"].as_str().unwrap_or("");
        let working_dir = self.params.get("working_dir").and_then(|v| v.as_str());

        let mut args = vec![Span::styled(command.to_string(), Style::default().fg(Color::White))];
        if let Some(dir) = working_dir {
            args.push(Span::styled(format!(", in {}", dir), Style::default().fg(Color::DarkGray)));
        }
        render_tool_block(self.status, self.background, "shell", args, &self.text, 10)
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
    use crate::transcript::lines_to_string;

    // =========================================================================
    // Render tests
    // =========================================================================

    #[test]
    fn test_render_pending() {
        let block = ShellBlock::new("call_1", "mcp_shell", json!({"command": "ls -la"}), false);
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? shell(ls -la)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_pending_with_working_dir() {
        let block = ShellBlock::new(
            "call_1",
            "mcp_shell",
            json!({"command": "ls -la", "working_dir": "/home/user"}),
            false,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? shell(ls -la, in /home/user)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_running() {
        let mut block = ShellBlock::new("call_1", "mcp_shell", json!({"command": "cargo build"}), false);
        block.status = Status::Running;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⚙ shell(cargo build)");
    }

    #[test]
    fn test_render_complete_with_output() {
        let mut block = ShellBlock::new("call_1", "mcp_shell", json!({"command": "echo hello"}), false);
        block.status = Status::Complete;
        block.text = "hello".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✓ shell(echo hello)\n  hello");
    }

    #[test]
    fn test_render_complete_with_multiline_output() {
        let mut block = ShellBlock::new("call_1", "mcp_shell", json!({"command": "ls"}), false);
        block.status = Status::Complete;
        block.text = "file1.txt\nfile2.txt\nfile3.txt".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✓ shell(ls)\n  file1.txt\n  file2.txt\n  file3.txt");
    }

    #[test]
    fn test_render_denied() {
        let mut block = ShellBlock::new("call_1", "mcp_shell", json!({"command": "rm -rf /"}), false);
        block.status = Status::Denied;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⊘ shell(rm -rf /)\n  Denied by user");
    }

    #[test]
    fn test_render_background() {
        let block = ShellBlock::new("call_1", "mcp_shell", json!({"command": "make build"}), true);
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? [bg] shell(make build)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_error() {
        let mut block = ShellBlock::new("call_1", "mcp_shell", json!({"command": "invalid_cmd"}), false);
        block.status = Status::Error;
        block.text = "Command not found".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✗ shell(invalid_cmd)\n  Command not found");
    }

    // =========================================================================
    // Execution tests
    // =========================================================================

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
