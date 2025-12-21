//! Task tool for spawning background agents

use super::{once_ready, Tool, ToolOutput, ToolResult};
use crate::impl_base_block;
use crate::tools::ToolEffect;
use crate::transcript::{render_approval_prompt, Block, BlockType, ToolBlock, Status};
use futures::stream::BoxStream;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Task block - shows the task description
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl TaskBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
        }
    }

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value) -> Option<Self> {
        let _: TaskParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for TaskBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let task = self.params["task"].as_str().unwrap_or("");
        // Truncate task for display
        let task_display = if task.len() > 60 {
            format!("{}...", &task[..57])
        } else {
            task.to_string()
        };

        lines.push(Line::from(vec![
            self.render_status(),
            Span::styled("task", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(task_display, Style::default().fg(Color::Yellow)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("  {}", self.text),
                Style::default().fg(Color::DarkGray),
            )));
        }

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

/// Tool for spawning background agents to handle subtasks
pub struct TaskTool;

#[derive(Debug, Deserialize)]
struct TaskParams {
    /// Description of the task for the sub-agent
    task: String,
    /// Optional context to provide to the sub-agent
    context: Option<String>,
}

impl Tool for TaskTool {
    fn name(&self) -> &'static str {
        "task"
    }

    fn description(&self) -> &'static str {
        "Spawn a background agent to handle a subtask. The sub-agent has read-only tool access \
         (read_file, shell, fetch_url, web_search) and will return its findings. \
         Use this for research, exploration, or analysis tasks that don't require file modifications."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Clear description of what the sub-agent should accomplish"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context or background information for the sub-agent"
                }
            },
            "required": ["task"]
        })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = TaskBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }

    fn execute(&self, params: serde_json::Value) -> BoxStream<'static, ToolOutput> {
        let params: TaskParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return once_ready(Ok(ToolResult::error(format!("Invalid params: {}", e)))),
        };

        // Create the effect to spawn a background agent
        let effect = ToolEffect::SpawnAgent {
            task: params.task.clone(),
            context: params.context,
        };

        // Return result with the spawn effect
        let result = ToolResult::success(format!("Spawning background agent for task: {}", params.task))
            .with_effects(vec![effect]);

        once_ready(Ok(result))
    }
}
