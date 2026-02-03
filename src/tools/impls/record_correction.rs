//! Record correction tool
//!
//! Allows the agent to record corrections when a shell command fails and
//! a subsequent approach succeeds. These corrections are stored in
//! `.codey/corrections.md` and loaded into the system prompt to help
//! the agent avoid repeating the same mistakes.

use super::{handlers, Tool, ToolPipeline};
use crate::define_tool_block;
use crate::transcript::{
    render_agent_label, render_approval_prompt, render_prefix, render_result, Block, BlockType,
    Status, ToolBlock,
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

define_tool_block! {
    /// Record correction display block
    pub struct RecordCorrectionBlock {
        max_lines: 3,
        params_type: RecordCorrectionParams,
        render_header(self, params) {
            let goal = params["goal"].as_str().unwrap_or("");
            let truncated: String = goal.chars().take(40).collect();
            let display = if truncated.len() < goal.len() {
                format!("{}...", truncated)
            } else {
                truncated
            };

            vec![
                Span::styled("record_correction", Style::default().fg(Color::Magenta)),
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled(display, Style::default().fg(Color::Green)),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]
        }
    }
}

/// Tool for recording corrections when shell commands fail and a better approach is found
pub struct RecordCorrectionTool;

#[derive(Debug, Deserialize)]
struct RecordCorrectionParams {
    /// What the agent was trying to accomplish
    goal: String,
    /// The command or approach that failed
    failed_attempt: String,
    /// The command or approach that succeeded
    successful_approach: String,
}

impl RecordCorrectionTool {
    pub const NAME: &'static str = "mcp_record_correction";
}

impl Tool for RecordCorrectionTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Record a correction when a shell command or approach fails and you find a better way. \
         This helps avoid repeating the same mistakes in future sessions."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "Brief description of what you were trying to accomplish (1-2 sentences)"
                },
                "failed_attempt": {
                    "type": "string",
                    "description": "The command or approach that didn't work"
                },
                "successful_approach": {
                    "type": "string",
                    "description": "The command or approach that worked instead"
                }
            },
            "required": ["goal", "failed_attempt", "successful_approach"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: Result<RecordCorrectionParams, _> = serde_json::from_value(params.clone());
        let params = match parsed {
            Ok(p) => p,
            Err(e) => {
                return ToolPipeline::error(format!("Invalid params: {}", e));
            }
        };

        ToolPipeline::new().then(handlers::AppendCorrection {
            goal: params.goal,
            failed_attempt: params.failed_attempt,
            successful_approach: params.successful_approach,
        })
    }

    fn create_block(
        &self,
        call_id: &str,
        params: serde_json::Value,
        background: bool,
    ) -> Box<dyn Block> {
        if let Some(block) =
            RecordCorrectionBlock::from_params(call_id, self.name(), params.clone(), background)
        {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params, background))
        }
    }
}
