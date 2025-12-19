//! Brave Web Search tool

use super::{Tool, ToolResult};
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_result, Block, BlockType, Status, ToolBlock};
use anyhow::Result;
use async_trait::async_trait;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Web Search display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl WebSearchBlock {
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
        let _: WebSearchParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for WebSearchBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let query = self.params["query"].as_str().unwrap_or("");

        // Format: web_search(query)
        lines.push(Line::from(vec![
            self.render_status(),
            Span::styled("web_search", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("\"{}\"", query), Style::default().fg(Color::Green)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.extend(render_result(
                &format!("{} results.", self.text.split("\n").count()), 1));
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

/// Tool for performing web searches using Brave Search API
pub struct WebSearchTool {
    client: reqwest::Client,
    timeout_secs: u64,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            timeout_secs: 30,
        }
    }

    fn get_api_key() -> Option<String> {
        std::env::var("BRAVE_API_KEY").ok()
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct WebSearchParams {
    query: String,
    #[serde(default = "default_count")]
    count: u32,
}

fn default_count() -> u32 {
    10
}

/// Brave Search API response structures
#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<WebResults>,
    #[serde(default)]
    query: Option<QueryInfo>,
}

#[derive(Debug, Deserialize)]
struct QueryInfo {
    #[serde(default)]
    original: String,
}

#[derive(Debug, Deserialize)]
struct WebResults {
    #[serde(default)]
    results: Vec<WebResult>,
}

#[derive(Debug, Deserialize)]
struct WebResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }

    fn description(&self) -> &'static str {
        "Returns relevant web results with titles, URLs, and descriptions."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of results to return (default: 10, max: 20)"
                }
            },
            "required": ["query"]
        })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = WebSearchBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let params: WebSearchParams = serde_json::from_value(params)?;
        let count = params.count.min(20); // Cap at 20 results

        // Get API key
        let api_key = match Self::get_api_key() {
            Some(key) => key,
            None => {
                return Ok(ToolResult::error(
                    "BRAVE_API_KEY environment variable not set. \
                     Get an API key from https://brave.com/search/api/",
                ));
            }
        };

        // Build request URL
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding::encode(&params.query),
            count
        );

        // Make request with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            self.client
                .get(&url)
                .header("Accept", "application/json")
                .header("X-Subscription-Token", &api_key)
                .send(),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                let status = response.status();

                if !status.is_success() {
                    let error_text = response.text().await.unwrap_or_default();
                    return Ok(ToolResult::error(format!(
                        "Brave Search API error: {} {} - {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown"),
                        error_text
                    )));
                }

                // Parse response
                match response.json::<BraveSearchResponse>().await {
                    Ok(search_response) => {
                        let mut output = String::new();

                        // Format results
                        if let Some(web) = search_response.web {
                            if web.results.is_empty() {
                                output.push_str("No results found.");
                            } else {
                                for (i, result) in web.results.iter().enumerate() {
                                    output.push_str(&format!(
                                        "{}. [{}]({})\n", i + 1, result.title, result.url));
                                }
                            }
                        } else {
                            output.push_str("No web results found.");
                        }

                        Ok(ToolResult::success(output))
                    }
                    Err(e) => Ok(ToolResult::error(format!(
                        "Failed to parse Brave Search response: {}",
                        e
                    ))),
                }
            }
            Ok(Err(e)) => Ok(ToolResult::error(format!("Request failed: {}", e))),
            Err(_) => Ok(ToolResult::error(format!(
                "Request timed out after {} seconds",
                self.timeout_secs
            ))),
        }
    }
}

