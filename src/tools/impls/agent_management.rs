//! Agent management tools
//!
//! Tools for querying and retrieving results from spawned sub-agents.

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
// ListAgents block
// =============================================================================

/// Block for list_agents - shows as `list_agents()`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListAgentsBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl ListAgentsBlock {
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
impl Block for ListAgentsBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("list_agents", Style::default().fg(Color::Magenta)),
            Span::styled("()", Style::default().fg(Color::DarkGray)),
        ];
        lines.push(Line::from(spans));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
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

// =============================================================================
// GetAgent block
// =============================================================================

/// Block for get_agent - shows as `get_agent(agent_id)`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAgentBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl GetAgentBlock {
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
impl Block for GetAgentBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let agent_id = self.params["agent_id"].as_u64()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "?".to_string());

        let spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("get_agent", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(agent_id, Style::default().fg(Color::White)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ];
        lines.push(Line::from(spans));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
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

// =============================================================================
// list_agents tool
// =============================================================================

/// Tool for listing all spawned sub-agents
pub struct ListAgentsTool;

impl ListAgentsTool {
    pub const NAME: &'static str = "mcp_list_agents";
}

impl Tool for ListAgentsTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "List all spawned sub-agents and their status. Returns agent IDs, labels, and status \
         (Running, Finished, or Error). Use get_agent to retrieve results from finished agents."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn compose(&self, _params: serde_json::Value) -> ToolPipeline {
        ToolPipeline::new()
            .await_approval()
            .then(handlers::ListAgents)
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(ListAgentsBlock::new(call_id, self.name(), params, background))
    }
}

// =============================================================================
// get_agent tool
// =============================================================================

/// Tool for retrieving a specific sub-agent's result
pub struct GetAgentTool;

#[derive(Debug, Deserialize)]
struct GetAgentParams {
    /// The agent_id returned when the agent was spawned
    agent_id: u32,
}

impl GetAgentTool {
    pub const NAME: &'static str = "mcp_get_agent";
}

impl Tool for GetAgentTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Retrieve the result of a finished sub-agent by its agent_id. \
         Returns the agent's final message/output. \
         If the agent is still running, returns its current status."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "integer",
                    "description": "The agent_id returned when the agent was spawned"
                }
            },
            "required": ["agent_id"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: GetAgentParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        ToolPipeline::new()
            .await_approval()
            .then(handlers::GetAgent {
                agent_id: parsed.agent_id,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(GetAgentBlock::new(call_id, self.name(), params, background))
    }
}
