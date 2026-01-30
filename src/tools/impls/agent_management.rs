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
use crate::define_simple_tool_block;
use crate::transcript::{render_approval_prompt, render_prefix, render_result, Block, BlockType, Status};

// =============================================================================
// ListAgents block
// =============================================================================

define_simple_tool_block! {
    /// Block for list_agents - shows as `list_agents()`
    pub struct ListAgentsBlock {
        max_lines: 10,
        render_header(self, params) {
            vec![
                Span::styled("list_agents", Style::default().fg(Color::Magenta)),
                Span::styled("()", Style::default().fg(Color::DarkGray)),
            ]
        }
    }
}

// =============================================================================
// GetAgent block
// =============================================================================

define_simple_tool_block! {
    /// Block for get_agent - shows as `get_agent(agent_id)`
    pub struct GetAgentBlock {
        max_lines: 10,
        render_header(self, params) {
            let label = params["label"].as_str().unwrap_or("?");

            vec![
                Span::styled("get_agent", Style::default().fg(Color::Magenta)),
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled(label.to_string(), Style::default().fg(Color::Yellow)),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]
        }
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
    /// The label of the agent to retrieve
    label: String,
}

impl GetAgentTool {
    pub const NAME: &'static str = "mcp_get_agent";
}

impl Tool for GetAgentTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Retrieve the result of a finished sub-agent by its label. \
         Returns the agent's final message/output. \
         If the agent is still running, returns its current status."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "label": {
                    "type": "string",
                    "description": "The label of the agent to retrieve"
                }
            },
            "required": ["label"]
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
                label: parsed.label,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(GetAgentBlock::new(call_id, self.name(), params, background))
    }
}
