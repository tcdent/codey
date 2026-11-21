//! Agent management tools
//!
//! Tools for querying spawned sub-agents.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::define_simple_tool_block;
use crate::transcript::{render_prefix, render_result, Block, BlockType, Status};

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
         (Running, Finished, or Error). Note: you are automatically notified when agents finish, \
         so you rarely need this. Only use to debug issues or if the user asks about agent status."
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
            .then(handlers::ListAgents)
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(ListAgentsBlock::new(call_id, self.name(), params, background))
    }
}
