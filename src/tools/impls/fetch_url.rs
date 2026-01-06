//! URL fetching tool

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_result, Block, BlockType, ToolBlock, Status};
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
}

impl FetchUrlBlock {
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
        let _: FetchUrlParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for FetchUrlBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let url = self.params["url"].as_str().unwrap_or("");

        // Format: fetch_url(url)
        lines.push(Line::from(vec![
            self.render_status(),
            Span::styled("fetch_url", Style::default().fg(Color::Magenta)),
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

/// Tool for fetching web content
pub struct FetchUrlTool;

#[derive(Debug, Deserialize)]
struct FetchUrlParams {
    url: String,
    max_length: Option<usize>,
}

impl Tool for FetchUrlTool {
    fn name(&self) -> &'static str {
        "fetch_url"
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

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = FetchUrlBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
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
            name: "fetch_url".to_string(),
            params: json!({ "url": "not a valid url" }),
            decision: ToolDecision::Approve,
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
            name: "fetch_url".to_string(),
            params: json!({ "url": "ftp://example.com/file" }),
            decision: ToolDecision::Approve,
        }]);

        if let Some(crate::tools::ToolEvent::Error { content, .. }) = executor.next().await {
            assert!(content.contains("Unsupported URL scheme"));
        } else {
            panic!("Expected Error event");
        }
    }
}
