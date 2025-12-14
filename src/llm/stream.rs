//! SSE streaming handler for Anthropic API

use super::types::*;
use anyhow::{Context, Result};
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;

/// Events emitted during streaming
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Message started
    MessageStart {
        message_id: String,
    },
    /// Content block started (text or tool use)
    ContentBlockStart {
        index: usize,
        content_type: ContentBlockType,
    },
    /// Text delta
    TextDelta {
        index: usize,
        text: String,
    },
    /// Tool use input delta (JSON string chunk)
    InputJsonDelta {
        index: usize,
        partial_json: String,
    },
    /// Content block finished
    ContentBlockStop {
        index: usize,
    },
    /// Message delta (stop reason, usage)
    MessageDelta {
        stop_reason: Option<StopReason>,
        usage: Option<Usage>,
    },
    /// Message finished
    MessageStop,
    /// Error occurred
    Error {
        message: String,
    },
}

/// Content block type
#[derive(Debug, Clone)]
pub enum ContentBlockType {
    Text,
    ToolUse { id: String, name: String },
}

/// Raw SSE event data structures
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum RawStreamEvent {
    MessageStart {
        message: RawMessageStart,
    },
    ContentBlockStart {
        index: usize,
        content_block: RawContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: RawDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: RawMessageDelta,
        usage: Option<Usage>,
    },
    MessageStop,
    Ping,
    Error {
        error: RawError,
    },
}

#[derive(Debug, Deserialize)]
struct RawMessageStart {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum RawContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum RawDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
}

#[derive(Debug, Deserialize)]
struct RawMessageDelta {
    stop_reason: Option<StopReason>,
}

#[derive(Debug, Deserialize)]
struct RawError {
    message: String,
}

/// Stream handler for processing SSE events
pub struct StreamHandler {
    event_tx: mpsc::Sender<StreamEvent>,
}

impl StreamHandler {
    /// Create a new stream handler
    pub fn new(event_tx: mpsc::Sender<StreamEvent>) -> Self {
        Self { event_tx }
    }

    /// Process a streaming response
    pub async fn process(&self, response: reqwest::Response) -> Result<()> {
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Failed to read stream chunk")?;
            let chunk_str = String::from_utf8_lossy(&chunk);
            buffer.push_str(&chunk_str);

            // Process complete events from buffer
            while let Some(event) = self.extract_event(&mut buffer) {
                if let Err(e) = self.event_tx.send(event).await {
                    tracing::warn!("Failed to send stream event: {}", e);
                    return Ok(());
                }
            }
        }

        Ok(())
    }

    /// Extract a complete event from the buffer
    fn extract_event(&self, buffer: &mut String) -> Option<StreamEvent> {
        // Find event boundary (double newline)
        let event_end = buffer.find("\n\n")?;
        let event_str = buffer[..event_end].to_string();
        *buffer = buffer[event_end + 2..].to_string();

        self.parse_event(&event_str)
    }

    /// Parse an SSE event string
    fn parse_event(&self, event_str: &str) -> Option<StreamEvent> {
        let mut event_type = None;
        let mut data = None;

        for line in event_str.lines() {
            if let Some(value) = line.strip_prefix("event: ") {
                event_type = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("data: ") {
                data = Some(value.to_string());
            }
        }

        let data = data?;

        // Parse based on event type
        match event_type.as_deref() {
            Some("message_start") | Some("content_block_start") | Some("content_block_delta")
            | Some("content_block_stop") | Some("message_delta") | Some("message_stop")
            | Some("ping") | Some("error") => {
                self.parse_data_event(&data)
            }
            _ => None,
        }
    }

    /// Parse the data portion of an event
    fn parse_data_event(&self, data: &str) -> Option<StreamEvent> {
        let raw: RawStreamEvent = serde_json::from_str(data).ok()?;

        match raw {
            RawStreamEvent::MessageStart { message } => Some(StreamEvent::MessageStart {
                message_id: message.id,
            }),
            RawStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let content_type = match content_block {
                    RawContentBlock::Text { .. } => ContentBlockType::Text,
                    RawContentBlock::ToolUse { id, name } => ContentBlockType::ToolUse { id, name },
                };
                Some(StreamEvent::ContentBlockStart {
                    index,
                    content_type,
                })
            }
            RawStreamEvent::ContentBlockDelta { index, delta } => match delta {
                RawDelta::TextDelta { text } => Some(StreamEvent::TextDelta { index, text }),
                RawDelta::InputJsonDelta { partial_json } => {
                    Some(StreamEvent::InputJsonDelta {
                        index,
                        partial_json,
                    })
                }
            },
            RawStreamEvent::ContentBlockStop { index } => {
                Some(StreamEvent::ContentBlockStop { index })
            }
            RawStreamEvent::MessageDelta { delta, usage } => Some(StreamEvent::MessageDelta {
                stop_reason: delta.stop_reason,
                usage,
            }),
            RawStreamEvent::MessageStop => Some(StreamEvent::MessageStop),
            RawStreamEvent::Ping => None, // Ignore pings
            RawStreamEvent::Error { error } => Some(StreamEvent::Error {
                message: error.message,
            }),
        }
    }
}

/// Builder for accumulating streamed content
#[derive(Debug, Default)]
pub struct StreamedMessage {
    pub message_id: Option<String>,
    pub content_blocks: Vec<ContentBlockBuilder>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

#[derive(Debug)]
pub enum ContentBlockBuilder {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input_json: String,
    },
}

impl StreamedMessage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a stream event to build the message
    pub fn apply_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::MessageStart { message_id } => {
                self.message_id = Some(message_id);
            }
            StreamEvent::ContentBlockStart {
                index,
                content_type,
            } => {
                // Ensure we have enough space
                while self.content_blocks.len() <= index {
                    self.content_blocks.push(ContentBlockBuilder::Text(String::new()));
                }

                self.content_blocks[index] = match content_type {
                    ContentBlockType::Text => ContentBlockBuilder::Text(String::new()),
                    ContentBlockType::ToolUse { id, name } => ContentBlockBuilder::ToolUse {
                        id,
                        name,
                        input_json: String::new(),
                    },
                };
            }
            StreamEvent::TextDelta { index, text } => {
                if let Some(ContentBlockBuilder::Text(ref mut s)) = self.content_blocks.get_mut(index)
                {
                    s.push_str(&text);
                }
            }
            StreamEvent::InputJsonDelta {
                index,
                partial_json,
            } => {
                if let Some(ContentBlockBuilder::ToolUse {
                    ref mut input_json, ..
                }) = self.content_blocks.get_mut(index)
                {
                    input_json.push_str(&partial_json);
                }
            }
            StreamEvent::ContentBlockStop { .. } => {}
            StreamEvent::MessageDelta { stop_reason, usage } => {
                self.stop_reason = stop_reason;
                if let Some(u) = usage {
                    self.usage = u;
                }
            }
            StreamEvent::MessageStop => {}
            StreamEvent::Error { .. } => {}
        }
    }

    /// Convert to a final message content vector
    pub fn into_content(self) -> Vec<Content> {
        self.content_blocks
            .into_iter()
            .filter_map(|block| match block {
                ContentBlockBuilder::Text(text) if !text.is_empty() => {
                    Some(Content::Text { text })
                }
                ContentBlockBuilder::Text(_) => None,
                ContentBlockBuilder::ToolUse {
                    id,
                    name,
                    input_json,
                } => {
                    let input = serde_json::from_str(&input_json).unwrap_or(serde_json::Value::Null);
                    Some(Content::ToolUse(ToolUse { id, name, input }))
                }
            })
            .collect()
    }

    /// Get the current text content
    pub fn current_text(&self) -> String {
        self.content_blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlockBuilder::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
