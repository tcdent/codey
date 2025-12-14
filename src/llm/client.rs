//! Anthropic API client

use super::types::*;
use crate::auth::Credentials;
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};

const API_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

/// Anthropic API client
pub struct AnthropicClient {
    http_client: reqwest::Client,
    credentials: Credentials,
    model: String,
    max_tokens: u32,
    system_prompt: Option<String>,
}

impl AnthropicClient {
    /// Create a new client with the given credentials
    pub fn new(credentials: Credentials, model: String, max_tokens: u32) -> Result<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            http_client,
            credentials,
            model,
            max_tokens,
            system_prompt: None,
        })
    }

    /// Set the system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Get the model name
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Build headers for API requests
    fn headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();

        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(API_VERSION),
        );

        // Add authentication header
        if self.credentials.is_api_key() {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(&self.credentials.auth_header())
                    .context("Invalid API key")?,
            );
        } else {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&self.credentials.auth_header())
                    .context("Invalid OAuth token")?,
            );
        }

        Ok(headers)
    }

    /// Create a message (non-streaming)
    pub async fn create_message(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<CreateMessageResponse> {
        let request = CreateMessageRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: self.system_prompt.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            stream: false,
        };

        let response = self
            .http_client
            .post(format!("{}/v1/messages", API_BASE_URL))
            .headers(self.headers()?)
            .json(&request)
            .send()
            .await
            .context("Failed to send request")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();

            if let Ok(api_error) = serde_json::from_str::<ApiError>(&error_text) {
                anyhow::bail!("API error ({}): {}", status, api_error.message);
            } else {
                anyhow::bail!("API error ({}): {}", status, error_text);
            }
        }

        response
            .json()
            .await
            .context("Failed to parse API response")
    }

    /// Create a streaming message request
    pub async fn create_message_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<reqwest::Response> {
        let request = CreateMessageRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            system: self.system_prompt.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            stream: true,
        };

        let response = self
            .http_client
            .post(format!("{}/v1/messages", API_BASE_URL))
            .headers(self.headers()?)
            .json(&request)
            .send()
            .await
            .context("Failed to send streaming request")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();

            if let Ok(api_error) = serde_json::from_str::<ApiError>(&error_text) {
                anyhow::bail!("API error ({}): {}", status, api_error.message);
            } else {
                anyhow::bail!("API error ({}): {}", status, error_text);
            }
        }

        Ok(response)
    }

    /// Update credentials (e.g., after token refresh)
    pub fn update_credentials(&mut self, credentials: Credentials) {
        self.credentials = credentials;
    }
}
