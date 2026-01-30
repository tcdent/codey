//! HTML content fetching tool with reader view
//!
//! Fetches web pages using a headless browser, extracts readable content
//! using the readability algorithm, and converts to markdown.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::define_tool_block;
use crate::transcript::{
    render_agent_label, render_approval_prompt, render_prefix, render_result, Block, BlockType, Status, ToolBlock,
};

define_tool_block! {
    /// Fetch HTML display block
    pub struct FetchHtmlBlock {
        max_lines: 5,
        params_type: FetchHtmlParams,
        render_header(self, params) {
            let url = params["url"].as_str().unwrap_or("");

            vec![
                Span::styled("fetch_html", Style::default().fg(Color::Magenta)),
                Span::styled("(", Style::default().fg(Color::DarkGray)),
                Span::styled(url.to_string(), Style::default().fg(Color::Blue)),
                Span::styled(")", Style::default().fg(Color::DarkGray)),
            ]
        }
    }
}

/// Tool for fetching web content with reader view
pub struct FetchHtmlTool;

#[derive(Debug, Deserialize)]
struct FetchHtmlParams {
    url: String,
    max_length: Option<usize>,
}

impl FetchHtmlTool {
    pub const NAME: &'static str = "mcp_fetch_html";
}

impl Tool for FetchHtmlTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Fetch a web page and extract readable content as markdown. \
        Use this tool when you encounter html pages or SPA applicaitons to extract the main content. \
        Save context by using this tool whenever you encounter URLs you expect to be html pages."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL of the web page to fetch"
                },
                "max_length": {
                    "type": "integer",
                    "description": "Maximum content length in characters (default: 100000)"
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
        let parsed: FetchHtmlParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        ToolPipeline::new()
            .await_approval()
            .then(handlers::FetchHtml {
                url: parsed.url,
                max_length: parsed.max_length,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        if let Some(block) = FetchHtmlBlock::from_params(call_id, self.name(), params.clone(), background) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params, background))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolCall, ToolDecision, ToolExecutor, ToolRegistry};

    #[tokio::test]
    async fn test_fetch_html_invalid_url() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(FetchHtmlTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: FetchHtmlTool::NAME.to_string(),
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
    async fn test_fetch_html_unsupported_scheme() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(FetchHtmlTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: FetchHtmlTool::NAME.to_string(),
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
