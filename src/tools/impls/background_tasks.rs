//! Background task management tools
//!
//! Tools for querying and retrieving results from background tool executions.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_prefix, render_result, Block, BlockType, Status};

// =============================================================================
// ListBackgroundTasks block
// =============================================================================

/// Block for list_background_tasks - shows as `list_background_tasks()`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListBackgroundTasksBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl ListBackgroundTasksBlock {
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
}

#[typetag::serde]
impl Block for ListBackgroundTasksBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        // Format: list_background_tasks()
        let spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("list_background_tasks", Style::default().fg(Color::Magenta)),
            Span::styled("()", Style::default().fg(Color::DarkGray)),
        ];
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

// =============================================================================
// GetBackgroundTask block
// =============================================================================

/// Block for get_background_task - shows as `get_background_task(task_id)`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBackgroundTaskBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl GetBackgroundTaskBlock {
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
}

#[typetag::serde]
impl Block for GetBackgroundTaskBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let task_id = self.params["task_id"].as_str().unwrap_or("");

        // Format: get_background_task(task_id)
        let spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("get_background_task", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(task_id, Style::default().fg(Color::White)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ];
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

// =============================================================================
// list_background_tasks tool
// =============================================================================

/// Tool for listing all background tasks
pub struct ListBackgroundTasksTool;

impl ListBackgroundTasksTool {
    pub const NAME: &'static str = "mcp_list_background_tasks";
}

impl Tool for ListBackgroundTasksTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "List all background tasks and their status. Returns task IDs, tool names, and status \
         (Running, Complete, or Error). Use get_background_task to retrieve results."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn compose(&self, _params: serde_json::Value) -> ToolPipeline {
        // TODO: Add a Started event so tools without approval still render
        ToolPipeline::new()
            .await_approval()
            .then(handlers::ListBackgroundTasks)
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(ListBackgroundTasksBlock::new(call_id, self.name(), params, background))
    }
}

// =============================================================================
// get_background_task tool
// =============================================================================

/// Tool for retrieving a specific background task result
pub struct GetBackgroundTaskTool;

#[derive(Debug, Deserialize)]
struct GetBackgroundTaskParams {
    /// The task_id returned when the background task was started
    task_id: String,
}

impl GetBackgroundTaskTool {
    pub const NAME: &'static str = "mcp_get_background_task";
}

impl Tool for GetBackgroundTaskTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Retrieve the result of a completed background task by its task_id. \
         The result is removed from tracking after retrieval. \
         Returns an error if the task is still running or doesn't exist."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The task_id returned when the background task was started"
                }
            },
            "required": ["task_id"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: GetBackgroundTaskParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        // TODO: Add a Started event so tools without approval still render
        ToolPipeline::new()
            .await_approval()
            .then(handlers::GetBackgroundTask {
                task_id: parsed.task_id,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(GetBackgroundTaskBlock::new(call_id, self.name(), params, background))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::lines_to_string;
    use serde_json::json;

    // =========================================================================
    // ListBackgroundTasksBlock render tests
    // =========================================================================

    #[test]
    fn test_list_render_pending() {
        let block = ListBackgroundTasksBlock::new(
            "call_1",
            "mcp_list_background_tasks",
            json!({}),
            false,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? list_background_tasks()\n  [y]es  [n]o");
    }

    #[test]
    fn test_list_render_running() {
        let mut block = ListBackgroundTasksBlock::new(
            "call_1",
            "mcp_list_background_tasks",
            json!({}),
            false,
        );
        block.status = Status::Running;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⚙ list_background_tasks()");
    }

    #[test]
    fn test_list_render_complete_with_output() {
        let mut block = ListBackgroundTasksBlock::new(
            "call_1",
            "mcp_list_background_tasks",
            json!({}),
            false,
        );
        block.status = Status::Complete;
        block.text = "task_1: shell (Complete)\ntask_2: read_file (Running)".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✓ list_background_tasks()\n  task_1: shell (Complete)\n  task_2: read_file (Running)");
    }

    #[test]
    fn test_list_render_denied() {
        let mut block = ListBackgroundTasksBlock::new(
            "call_1",
            "mcp_list_background_tasks",
            json!({}),
            false,
        );
        block.status = Status::Denied;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⊘ list_background_tasks()\n  Denied by user");
    }

    #[test]
    fn test_list_render_background() {
        let block = ListBackgroundTasksBlock::new(
            "call_1",
            "mcp_list_background_tasks",
            json!({}),
            true,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? [bg] list_background_tasks()\n  [y]es  [n]o");
    }

    // =========================================================================
    // GetBackgroundTaskBlock render tests
    // =========================================================================

    #[test]
    fn test_get_render_pending() {
        let block = GetBackgroundTaskBlock::new(
            "call_1",
            "mcp_get_background_task",
            json!({"task_id": "task_123"}),
            false,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? get_background_task(task_123)\n  [y]es  [n]o");
    }

    #[test]
    fn test_get_render_running() {
        let mut block = GetBackgroundTaskBlock::new(
            "call_1",
            "mcp_get_background_task",
            json!({"task_id": "task_123"}),
            false,
        );
        block.status = Status::Running;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⚙ get_background_task(task_123)");
    }

    #[test]
    fn test_get_render_complete_with_output() {
        let mut block = GetBackgroundTaskBlock::new(
            "call_1",
            "mcp_get_background_task",
            json!({"task_id": "task_123"}),
            false,
        );
        block.status = Status::Complete;
        block.text = "file1.txt\nfile2.txt".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✓ get_background_task(task_123)\n  file1.txt\n  file2.txt");
    }

    #[test]
    fn test_get_render_denied() {
        let mut block = GetBackgroundTaskBlock::new(
            "call_1",
            "mcp_get_background_task",
            json!({"task_id": "task_123"}),
            false,
        );
        block.status = Status::Denied;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⊘ get_background_task(task_123)\n  Denied by user");
    }

    #[test]
    fn test_get_render_background() {
        let block = GetBackgroundTaskBlock::new(
            "call_1",
            "mcp_get_background_task",
            json!({"task_id": "task_123"}),
            true,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? [bg] get_background_task(task_123)\n  [y]es  [n]o");
    }

    #[test]
    fn test_get_render_error() {
        let mut block = GetBackgroundTaskBlock::new(
            "call_1",
            "mcp_get_background_task",
            json!({"task_id": "task_123"}),
            false,
        );
        block.status = Status::Error;
        block.text = "Task not found".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✗ get_background_task(task_123)\n  Task not found");
    }
}
