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
use crate::impl_base_block;
use crate::transcript::{
    render_approval_prompt, render_prefix, render_result, Block, BlockType, Status, ToolBlock,
};

/// Fetch HTML display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchHtmlBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl FetchHtmlBlock {
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
        }
    }

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value, background: bool) -> Option<Self> {
        let _: FetchHtmlParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params, background))
    }
}

#[typetag::serde]
impl Block for FetchHtmlBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let url = self.params["url"].as_str().unwrap_or("");

        lines.push(Line::from(vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("fetch_html", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(url, Style::default().fg(Color::Blue)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
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
    use crate::transcript::lines_to_string;

    // =========================================================================
    // Render tests
    // =========================================================================

    #[test]
    fn test_render_pending() {
        let block = FetchHtmlBlock::new(
            "call_1",
            "mcp_fetch_html",
            json!({"url": "https://example.com"}),
            false,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? fetch_html(https://example.com)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_running() {
        let mut block = FetchHtmlBlock::new(
            "call_1",
            "mcp_fetch_html",
            json!({"url": "https://example.com"}),
            false,
        );
        block.status = Status::Running;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⚙ fetch_html(https://example.com)");
    }

    #[test]
    fn test_render_complete_with_output() {
        let mut block = FetchHtmlBlock::new(
            "call_1",
            "mcp_fetch_html",
            json!({"url": "https://example.com"}),
            false,
        );
        block.status = Status::Complete;
        // Use text without blank lines to avoid render_result preserving them
        block.text = "# Example Domain\nThis is an example.".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✓ fetch_html(https://example.com)\n  # Example Domain\n  This is an example.");
    }

    #[test]
    fn test_render_denied() {
        let mut block = FetchHtmlBlock::new(
            "call_1",
            "mcp_fetch_html",
            json!({"url": "https://example.com"}),
            false,
        );
        block.status = Status::Denied;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⊘ fetch_html(https://example.com)\n  Denied by user");
    }

    #[test]
    fn test_render_background() {
        let block = FetchHtmlBlock::new(
            "call_1",
            "mcp_fetch_html",
            json!({"url": "https://example.com"}),
            true,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? [bg] fetch_html(https://example.com)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_error() {
        let mut block = FetchHtmlBlock::new(
            "call_1",
            "mcp_fetch_html",
            json!({"url": "https://example.com"}),
            false,
        );
        block.status = Status::Error;
        block.text = "Page not found".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✗ fetch_html(https://example.com)\n  Page not found");
    }

    // =========================================================================
    // Execution tests
    // =========================================================================

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
