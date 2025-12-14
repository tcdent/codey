//! Agent loop for handling conversations with tool execution

use super::client::AnthropicClient;
use super::stream::{StreamEvent, StreamHandler, StreamedMessage};
use super::types::*;
use crate::tools::{ToolRegistry, ToolResult as ToolExecResult};
use crate::ui::{PermissionHandler, PermissionRequest, PermissionResponse, RiskLevel};
use anyhow::{Context, Result};
use tokio::sync::mpsc;

/// Agent for handling conversations
pub struct Agent {
    client: AnthropicClient,
    tools: ToolRegistry,
    messages: Vec<Message>,
    permission_handler: Box<dyn PermissionHandler>,
    total_usage: Usage,
}

/// Events emitted by the agent during processing
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Streaming text delta
    TextDelta(String),
    /// Full text message completed
    TextComplete(String),
    /// Tool use requested
    ToolRequested {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool execution started (after permission granted)
    ToolExecuting {
        id: String,
        name: String,
    },
    /// Tool execution completed
    ToolCompleted {
        id: String,
        name: String,
        result: String,
        is_error: bool,
    },
    /// Tool execution denied by user
    ToolDenied {
        id: String,
        name: String,
    },
    /// Agent finished processing
    Finished {
        usage: Usage,
    },
    /// Error occurred
    Error(String),
}

impl Agent {
    /// Create a new agent
    pub fn new(
        client: AnthropicClient,
        tools: ToolRegistry,
        permission_handler: Box<dyn PermissionHandler>,
    ) -> Self {
        Self {
            client,
            tools,
            messages: Vec::new(),
            permission_handler,
            total_usage: Usage::default(),
        }
    }

    /// Set the system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.client = self.client.with_system_prompt(prompt);
        self
    }

    /// Get the tool definitions for the API
    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.definitions()
    }

    /// Get total token usage
    pub fn total_usage(&self) -> Usage {
        self.total_usage
    }

    /// Get current messages
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Clear conversation history
    pub fn clear_history(&mut self) {
        self.messages.clear();
        self.total_usage = Usage::default();
    }

    /// Process a user message with streaming
    pub async fn process_message(
        &mut self,
        user_input: &str,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<()> {
        // Add user message
        self.messages.push(Message::user(user_input));

        // Main agent loop
        loop {
            // Get streaming response
            let response = self
                .client
                .create_message_stream(&self.messages, &self.tool_definitions())
                .await
                .context("Failed to create message")?;

            // Process stream
            let (stream_tx, mut stream_rx) = mpsc::channel::<StreamEvent>(100);
            let handler = StreamHandler::new(stream_tx);

            // Spawn stream processing task
            let stream_handle = tokio::spawn(async move {
                handler.process(response).await
            });

            // Build message from stream events
            let mut streamed_msg = StreamedMessage::new();

            while let Some(event) = stream_rx.recv().await {
                // Send text deltas to UI
                if let StreamEvent::TextDelta { ref text, .. } = event {
                    let _ = event_tx.send(AgentEvent::TextDelta(text.clone())).await;
                }

                if let StreamEvent::Error { ref message } = event {
                    let _ = event_tx
                        .send(AgentEvent::Error(message.clone()))
                        .await;
                    return Err(anyhow::anyhow!("Stream error: {}", message));
                }

                streamed_msg.apply_event(event);
            }

            // Wait for stream to complete
            stream_handle.await??;

            // Update usage
            self.total_usage += streamed_msg.usage;

            // Check stop reason and handle accordingly
            let stop_reason = streamed_msg.stop_reason;
            let content = streamed_msg.into_content();

            // Send text complete event
            let text: String = content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            if !text.is_empty() {
                let _ = event_tx.send(AgentEvent::TextComplete(text)).await;
            }

            // Add assistant message to history
            self.messages.push(Message::assistant(content.clone()));

            match stop_reason {
                Some(StopReason::EndTurn) => {
                    // Finished
                    let _ = event_tx
                        .send(AgentEvent::Finished {
                            usage: self.total_usage,
                        })
                        .await;
                    return Ok(());
                }
                Some(StopReason::ToolUse) => {
                    // Extract tool uses and execute
                    let tool_uses: Vec<ToolUse> = content
                        .iter()
                        .filter_map(|c| match c {
                            Content::ToolUse(tu) => Some(tu.clone()),
                            _ => None,
                        })
                        .collect();

                    let mut tool_results = Vec::new();

                    for tool_use in tool_uses {
                        // Notify UI of tool request
                        let _ = event_tx
                            .send(AgentEvent::ToolRequested {
                                id: tool_use.id.clone(),
                                name: tool_use.name.clone(),
                                input: tool_use.input.clone(),
                            })
                            .await;

                        // Request permission
                        let permission_request = PermissionRequest {
                            tool_name: tool_use.name.clone(),
                            params: tool_use.input.clone(),
                            description: self.format_tool_description(&tool_use),
                            risk_level: self.get_risk_level(&tool_use.name),
                        };

                        let permission = self
                            .permission_handler
                            .request_permission(permission_request)
                            .await;

                        match permission {
                            PermissionResponse::Allow | PermissionResponse::AllowOnce => {
                                // Execute tool
                                let _ = event_tx
                                    .send(AgentEvent::ToolExecuting {
                                        id: tool_use.id.clone(),
                                        name: tool_use.name.clone(),
                                    })
                                    .await;

                                let result = self
                                    .tools
                                    .execute(&tool_use.name, tool_use.input.clone())
                                    .await;

                                let (content, is_error) = match result {
                                    Ok(ToolExecResult { content, .. }) => (content, false),
                                    Err(e) => (format!("Error: {}", e), true),
                                };

                                let _ = event_tx
                                    .send(AgentEvent::ToolCompleted {
                                        id: tool_use.id.clone(),
                                        name: tool_use.name.clone(),
                                        result: content.clone(),
                                        is_error,
                                    })
                                    .await;

                                tool_results.push(ToolResult {
                                    tool_use_id: tool_use.id,
                                    content,
                                    is_error,
                                });
                            }
                            PermissionResponse::Deny => {
                                let _ = event_tx
                                    .send(AgentEvent::ToolDenied {
                                        id: tool_use.id.clone(),
                                        name: tool_use.name.clone(),
                                    })
                                    .await;

                                tool_results.push(ToolResult::error(
                                    tool_use.id,
                                    "User denied permission to execute this tool",
                                ));
                            }
                            PermissionResponse::AllowForSession => {
                                // TODO: Implement session-wide permission
                                // For now, treat as Allow
                                let result = self
                                    .tools
                                    .execute(&tool_use.name, tool_use.input.clone())
                                    .await;

                                let (content, is_error) = match result {
                                    Ok(ToolExecResult { content, .. }) => (content, false),
                                    Err(e) => (format!("Error: {}", e), true),
                                };

                                tool_results.push(ToolResult {
                                    tool_use_id: tool_use.id,
                                    content,
                                    is_error,
                                });
                            }
                        }
                    }

                    // Add tool results to messages
                    self.messages.push(Message::tool_results(tool_results));

                    // Continue loop to get next response
                }
                Some(StopReason::MaxTokens) => {
                    let _ = event_tx
                        .send(AgentEvent::Error(
                            "Response exceeded maximum tokens".to_string(),
                        ))
                        .await;
                    return Err(anyhow::anyhow!("Response exceeded maximum tokens"));
                }
                Some(StopReason::StopSequence) | None => {
                    let _ = event_tx
                        .send(AgentEvent::Finished {
                            usage: self.total_usage,
                        })
                        .await;
                    return Ok(());
                }
            }
        }
    }

    /// Format a human-readable description of a tool use
    fn format_tool_description(&self, tool_use: &ToolUse) -> String {
        match tool_use.name.as_str() {
            "read_file" => {
                let path = tool_use.input.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                format!("Read file: {}", path)
            }
            "write_file" => {
                let path = tool_use.input.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                format!("Write file: {}", path)
            }
            "edit_file" => {
                let path = tool_use.input.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                let edits = tool_use
                    .input
                    .get("edits")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                format!("Edit file: {} ({} edits)", path, edits)
            }
            "shell" => {
                let command = tool_use
                    .input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                format!("Execute: {}", command)
            }
            "fetch_url" => {
                let url = tool_use.input.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                format!("Fetch URL: {}", url)
            }
            _ => format!("{}: {:?}", tool_use.name, tool_use.input),
        }
    }

    /// Get the risk level for a tool
    fn get_risk_level(&self, tool_name: &str) -> RiskLevel {
        match tool_name {
            "read_file" | "fetch_url" => RiskLevel::Low,
            "write_file" | "edit_file" => RiskLevel::Medium,
            "shell" => RiskLevel::High,
            _ => RiskLevel::Medium,
        }
    }
}
