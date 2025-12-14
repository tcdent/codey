//! URL fetching tool

use super::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

/// Tool for fetching web content
pub struct FetchUrlTool {
    client: reqwest::Client,
    timeout_secs: u64,
}

impl FetchUrlTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Codepal/0.1")
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            timeout_secs: 30,
        }
    }
}

impl Default for FetchUrlTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct FetchUrlParams {
    url: String,
    max_length: Option<usize>,
}

#[async_trait]
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

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let params: FetchUrlParams = serde_json::from_value(params)?;
        let max_length = params.max_length.unwrap_or(50000);

        // Validate URL
        let url = match url::Url::parse(&params.url) {
            Ok(u) => u,
            Err(e) => {
                return Ok(ToolResult::error(format!("Invalid URL: {}", e)));
            }
        };

        // Only allow http/https
        if url.scheme() != "http" && url.scheme() != "https" {
            return Ok(ToolResult::error(format!(
                "Unsupported URL scheme: {}. Only http and https are allowed.",
                url.scheme()
            )));
        }

        // Fetch with timeout
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            self.client.get(url.as_str()).send(),
        )
        .await;

        match result {
            Ok(Ok(response)) => {
                let status = response.status();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("unknown")
                    .to_string();

                if !status.is_success() {
                    return Ok(ToolResult::error(format!(
                        "HTTP error: {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown")
                    )));
                }

                // Get response body
                match response.text().await {
                    Ok(mut text) => {
                        let original_len = text.len();

                        // Truncate if needed
                        if text.len() > max_length {
                            text = text[..max_length].to_string();
                            text.push_str(&format!(
                                "\n\n[... truncated, {} of {} bytes shown]",
                                max_length, original_len
                            ));
                        }

                        // Add metadata header
                        let header = format!(
                            "[URL: {}]\n[Content-Type: {}]\n[Size: {} bytes]\n\n",
                            params.url, content_type, original_len
                        );

                        Ok(ToolResult::success(header + &text))
                    }
                    Err(e) => Ok(ToolResult::error(format!(
                        "Failed to read response body: {}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_invalid_url() {
        let tool = FetchUrlTool::new();
        let result = tool
            .execute(json!({
                "url": "not a valid url"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("Invalid URL"));
    }

    #[tokio::test]
    async fn test_fetch_unsupported_scheme() {
        let tool = FetchUrlTool::new();
        let result = tool
            .execute(json!({
                "url": "ftp://example.com/file"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("Unsupported URL scheme"));
    }
}
