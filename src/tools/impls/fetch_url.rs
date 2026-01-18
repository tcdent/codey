//! URL fetching tool

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{render_tool_block, Block, BlockType, ToolBlock, Status};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Fetch URL display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchUrlBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl FetchUrlBlock {
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

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value, background: bool) -> Option<Self> {
        let _: FetchUrlParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params, background))
    }
}

#[typetag::serde]
impl Block for FetchUrlBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let url = self.params["url"].as_str().unwrap_or("");
        let args = vec![Span::styled(url.to_string(), Style::default().fg(Color::Blue))];
        render_tool_block(self.status, self.background, "fetch_url", args, &self.text, 5)
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
    use crate::transcript::lines_to_string;

    // =========================================================================
    // Render tests
    // =========================================================================

    #[test]
    fn test_render_pending() {
        let block = FetchUrlBlock::new(
            "call_1",
            "mcp_fetch_url",
            json!({"url": "https://example.com/api"}),
            false,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? fetch_url(https://example.com/api)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_running() {
        let mut block = FetchUrlBlock::new(
            "call_1",
            "mcp_fetch_url",
            json!({"url": "https://example.com/api"}),
            false,
        );
        block.status = Status::Running;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⚙ fetch_url(https://example.com/api)");
    }

    #[test]
    fn test_render_complete_with_output() {
        let mut block = FetchUrlBlock::new(
            "call_1",
            "mcp_fetch_url",
            json!({"url": "https://example.com/api"}),
            false,
        );
        block.status = Status::Complete;
        block.text = "{\"status\": \"ok\"}".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✓ fetch_url(https://example.com/api)\n  {\"status\": \"ok\"}");
    }

    #[test]
    fn test_render_denied() {
        let mut block = FetchUrlBlock::new(
            "call_1",
            "mcp_fetch_url",
            json!({"url": "https://example.com/api"}),
            false,
        );
        block.status = Status::Denied;
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "⊘ fetch_url(https://example.com/api)\n  Denied by user");
    }

    #[test]
    fn test_render_background() {
        let block = FetchUrlBlock::new(
            "call_1",
            "mcp_fetch_url",
            json!({"url": "https://example.com/api"}),
            true,
        );
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "? [bg] fetch_url(https://example.com/api)\n  [y]es  [n]o");
    }

    #[test]
    fn test_render_error() {
        let mut block = FetchUrlBlock::new(
            "call_1",
            "mcp_fetch_url",
            json!({"url": "https://example.com/api"}),
            false,
        );
        block.status = Status::Error;
        block.text = "Connection refused".to_string();
        let output = lines_to_string(&block.render(80));
        assert_eq!(output, "✗ fetch_url(https://example.com/api)\n  Connection refused");
    }

    // =========================================================================
    // Execution tests
    // =========================================================================

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
