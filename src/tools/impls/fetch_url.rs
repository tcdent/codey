//! URL fetching tool

use super::{handlers, Tool, ToolPipeline};
use crate::define_tool_block;
use crate::transcript::{render_agent_label, render_approval_prompt, render_prefix, render_result, Block, BlockType, ToolBlock, Status};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

define_tool_block! {
    /// Fetch URL display block
    pub struct FetchUrlBlock {
        max_lines: 5,
        params_type: FetchUrlParams,
        render_header(self, params) {
            let url = params["url"].as_str().unwrap_or("");

            vec![
                Span::styled("fetch_url", Style::default().fg(Color::Magenta)),
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled(url.to_string(), Style::default().fg(Color::Blue)),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]
        }
    }
}

/// Tool for fetching web content
pub struct FetchUrlTool;

#[derive(Debug, Deserialize)]
struct FetchUrlParams {
    url: String,
    max_length: Option<usize>,
}

impl FetchUrlTool {
    pub const NAME: &'static str = "mcp_fetch_url";
}

impl Tool for FetchUrlTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Fetch content from a URL. Returns text content (HTML, JSON, plain text). \
         Useful for documentation, API responses, web pages. \
         Content is truncated if it exceeds max_length."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum content length in characters (default: 50000)"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id; use list_background_tasks/get_background_task to check status and retrieve results."
                }
            },
            "required": ["url"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: FetchUrlParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        ToolPipeline::new()
            .await_approval()
            .then(handlers::FetchUrl {
                url: parsed.url,
                max_length: parsed.max_length,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        if let Some(block) = FetchUrlBlock::from_params(call_id, self.name(), params.clone(), background) {
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
    async fn test_fetch_invalid_url() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(FetchUrlTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: FetchUrlTool::NAME.to_string(),
            params: json!({ "url": "not a valid url" }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Error { content, .. }) = executor.next().await {
            assert!(content.contains("Invalid URL"));
        } else {
            panic!("Expected Error event");
        }
    }

    #[tokio::test]
    async fn test_fetch_unsupported_scheme() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(FetchUrlTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: FetchUrlTool::NAME.to_string(),
            params: json!({ "url": "ftp://example.com/file" }),
            decision: ToolDecision::Approve,
            background: false,
        }]);

        if let Some(crate::tools::ToolEvent::Error { content, .. }) = executor.next().await {
            assert!(content.contains("Unsupported URL scheme"));
        } else {
            panic!("Expected Error event");
        }
    }
}
