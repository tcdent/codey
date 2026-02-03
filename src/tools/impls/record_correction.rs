//! Record correction tool
//!
//! Allows the agent to record corrections when a shell command fails and
//! a subsequent approach succeeds. These corrections are stored in
//! `.codey/corrections.md` and loaded into the system prompt to help
//! the agent avoid repeating the same mistakes.

use super::{handlers, Tool, ToolPipeline};
use crate::impl_tool_block;
use crate::transcript::{
    render_agent_label, render_prefix, render_result, Block, BlockType, Status, ToolBlock,
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Record correction display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordCorrectionBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
    /// Agent label for sub-agent tools
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_label: Option<String>,
}

impl RecordCorrectionBlock {
    pub fn new(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        params: serde_json::Value,
        background: bool,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
            agent_label: None,
        }
    }

    pub fn from_params(
        call_id: &str,
        tool_name: &str,
        params: serde_json::Value,
        background: bool,
    ) -> Option<Self> {
        let _: RecordCorrectionParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params, background))
    }
}

#[typetag::serde]
impl Block for RecordCorrectionBlock {
    impl_tool_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let goal = self
            .params["goal"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect::<String>();

        // Format: [agent_label] record_correction(goal preview...)
        lines.push(Line::from(vec![
            self.render_status(),
            render_agent_label(self.agent_label.as_deref()),
            render_prefix(self.background),
            Span::styled("record_correction", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if goal.len() == 40 {
                    format!("{}...", goal)
                } else {
                    goal
                },
                Style::default().fg(Color::Green),
            ),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 3));
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

    fn set_agent_label(&mut self, label: String) {
        self.agent_label = Some(label);
    }

    fn agent_label(&self) -> Option<&str> {
        self.agent_label.as_deref()
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

        ToolPipeline::new()
            .then(handlers::AppendCorrection {
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
