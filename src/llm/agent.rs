//! Agent loop for handling conversations with tool execution

use crate::transcript::Block;
use crate::tools::ToolRegistry;
use anyhow::Result;
use futures::StreamExt;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent, ChatStreamResponse,
    ContentPart, MessageContent, Tool, ToolCall, ToolResponse,
};
use genai::Client;
use std::time::Duration;
use tracing::{debug, error, info};

/// Token usage tracking
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Steps yielded by the agent during processing
pub enum AgentStep {
    /// Streaming text chunk
    TextDelta(String),
    /// Agent wants to execute a tool, needs approval
    ToolRequest {
        call_id: String,
        block: Box<dyn Block>,
    },
    /// Tool finished executing
    ToolResult {
        call_id: String,
        result: String,
        is_error: bool,
    },
    /// Retrying after error
    Retrying { attempt: u32, error: String },
    /// Agent finished processing this message
    Finished { usage: Usage },
    /// Error occurred
    Error(String),
}

pub use crate::permission::ToolDecision;

/// Agent for handling conversations
pub struct Agent {
    client: Client,
    model: String,
    max_tokens: u32,
    max_retries: u32,
    tools: ToolRegistry,
    messages: Vec<ChatMessage>,
    total_usage: Usage,
}

impl Agent {
    /// Create a new agent with initial messages
    pub fn new(
        model: impl Into<String>,
        max_tokens: u32,
        max_retries: u32,
        messages: Vec<ChatMessage>,
        tools: ToolRegistry,
    ) -> Self {
        Self {
            client: Client::default(),
            model: model.into(),
            max_tokens,
            max_retries,
            tools,
            messages,
            total_usage: Usage::default(),
        }
    }

    /// Get tool definitions in genai format
    fn get_tools(&self) -> Vec<Tool> {
        self.tools
            .values()
            .map(|tool| {
                Tool::new(tool.name())
                    .with_description(tool.description())
                    .with_schema(tool.schema())
            })
            .collect()
    }

    /// Start processing a user message, returns a stream of steps
    pub fn process_message(&mut self, user_input: &str) -> AgentStream<'_> {
        self.messages.push(ChatMessage::user(user_input));
        AgentStream::new(self)
    }
}

/// Internal state for the agent stream
enum StreamState {
    /// Need to make a new chat API request
    NeedsChatRequest,
    /// Currently streaming response from API
    Streaming {
        stream: futures::stream::BoxStream<'static, Result<ChatStreamEvent, genai::Error>>,
        full_text: String,
        tool_calls: Vec<ToolCall>,
    },
    /// Waiting for tool approval decision
    AwaitingToolDecision {
        assistant_text: String,
        all_tool_calls: Vec<ToolCall>,
        current_tool_index: usize,
        tool_responses: Vec<ToolResponse>,
    },
    /// Processing completed
    Finished,
}

/// Stream that yields AgentSteps and accepts ToolDecisions
pub struct AgentStream<'a> {
    agent: &'a mut Agent,
    state: StreamState,
    tools: Vec<Tool>,
    chat_options: ChatOptions,
}

impl<'a> AgentStream<'a> {
    fn new(agent: &'a mut Agent) -> Self {
        let tools = agent.get_tools();
        let chat_options = ChatOptions::default()
            .with_max_tokens(agent.max_tokens)
            .with_capture_tool_calls(true);

        Self {
            agent,
            state: StreamState::NeedsChatRequest,
            tools,
            chat_options,
        }
    }

    /// Execute a chat request with retry and exponential backoff
    async fn exec_chat_with_retry(&self) -> Result<ChatStreamResponse, AgentStep> {
        let request =
            ChatRequest::new(self.agent.messages.clone())
                .with_tools(self.tools.clone());

        info!(
            "Making chat request with {} messages",
            self.agent.messages.len()
        );
        for (i, msg) in self.agent.messages.iter().enumerate() {
            debug!("Message {}: role={:?}, content={:?}", i, msg.role, msg.content);
        }

        let mut attempt = 0u32;
        let _delay = Duration::from_millis(500); // TODO: implement exponential backoff

        loop {
            attempt += 1;
            match self
                .agent
                .client
                .exec_chat_stream(&self.agent.model, request.clone(), Some(&self.chat_options))
                .await
            {
                Ok(resp) => {
                    info!("Chat request successful");
                    return Ok(resp);
                }
                Err(e) => {
                    let err = format!("{:#}", e);
                    error!("Chat request failed: {}", err);
                    if attempt >= self.agent.max_retries {
                        return Err(AgentStep::Error(format!(
                            "API error ({}): {}",
                            self.agent.model, err
                        )));
                    }
                    // Return retry step, caller should call next() again
                    return Err(AgentStep::Retrying { attempt, error: err });
                }
            }
        }
    }

    /// Get the next step from the agent
    pub async fn next(&mut self) -> Option<AgentStep> {
        loop {
            match &mut self.state {
                StreamState::NeedsChatRequest => {
                    match self.exec_chat_with_retry().await {
                        Ok(response) => {
                            self.state = StreamState::Streaming {
                                stream: Box::pin(response.stream),
                                full_text: String::new(),
                                tool_calls: Vec::new(),
                            };
                            // Continue to process streaming state
                        }
                        Err(step) => {
                            // Retrying or Error
                            if matches!(step, AgentStep::Retrying { .. }) {
                                // Stay in NeedsChatRequest, will retry on next call
                            }
                            return Some(step);
                        }
                    }
                }

                StreamState::Streaming {
                    stream,
                    full_text,
                    tool_calls,
                } => {
                    while let Some(result) = stream.next().await {
                        match result {
                            Ok(event) => match event {
                                ChatStreamEvent::Start => {}
                                ChatStreamEvent::Chunk(chunk) => {
                                    full_text.push_str(&chunk.content);
                                    return Some(AgentStep::TextDelta(chunk.content));
                                }
                                ChatStreamEvent::ToolCallChunk(_) => {}
                                ChatStreamEvent::ReasoningChunk(_) => {}
                                ChatStreamEvent::End(end) => {
                                    if let Some(ref usage) = end.captured_usage {
                                        self.agent.total_usage.input_tokens +=
                                            usage.prompt_tokens.unwrap_or(0) as u32;
                                        self.agent.total_usage.output_tokens +=
                                            usage.completion_tokens.unwrap_or(0) as u32;
                                    }
                                    if let Some(captured) = end.captured_into_tool_calls() {
                                        *tool_calls = captured;
                                    }
                                }
                            },
                            Err(e) => {
                                let err_msg = format!("Stream error: {:#}", e);
                                error!("Stream error: {:?}", e);
                                self.state = StreamState::Finished;
                                return Some(AgentStep::Error(err_msg));
                            }
                        }
                    }

                    // Stream ended, process results
                    let full_text = std::mem::take(full_text);
                    let tool_calls = std::mem::take(tool_calls);

                    if tool_calls.is_empty() {
                        // No tool calls, just add text message
                        self.agent.messages.push(ChatMessage::assistant(&full_text));
                        self.state = StreamState::Finished;
                        return Some(AgentStep::Finished {
                            usage: self.agent.total_usage,
                        });
                    }

                    // Start processing tool calls (don't add message yet - wait until all done)
                    self.state = StreamState::AwaitingToolDecision {
                        assistant_text: full_text,
                        all_tool_calls: tool_calls,
                        current_tool_index: 0,
                        tool_responses: Vec::new(),
                    };
                    // Continue to process tool decision state
                }

                StreamState::AwaitingToolDecision {
                    all_tool_calls,
                    current_tool_index,
                    ..
                } => {
                    // Yield the current tool request
                    let tool_call = &all_tool_calls[*current_tool_index];
                    let tool = self.agent.tools.get(&tool_call.fn_name);
                    let block = tool.create_block(&tool_call.call_id, tool_call.fn_arguments.clone());

                    return Some(AgentStep::ToolRequest {
                        call_id: tool_call.call_id.clone(),
                        block,
                    });
                }

                StreamState::Finished => {
                    return None;
                }
            }
        }
    }

    /// Respond to a tool approval request
    pub async fn decide_tool(&mut self, decision: ToolDecision) -> Option<AgentStep> {
        match std::mem::replace(&mut self.state, StreamState::Finished) {
            StreamState::AwaitingToolDecision {
                assistant_text,
                all_tool_calls,
                current_tool_index,
                mut tool_responses,
            } => {
                let tool_call = &all_tool_calls[current_tool_index];
                let tool = self.agent.tools.get(&tool_call.fn_name);
                let params = tool_call.fn_arguments.clone();

                let (content, is_error) = match decision {
                    ToolDecision::Approve => match tool.execute(params).await {
                        Ok(res) => (res.content, res.is_error),
                        Err(e) => (format!("Error: {}", e), true),
                    },
                    ToolDecision::Deny => ("Denied by user".to_string(), true),
                };

                let result_step = AgentStep::ToolResult {
                    call_id: tool_call.call_id.clone(),
                    result: content.clone(),
                    is_error,
                };

                tool_responses.push(ToolResponse::new(tool_call.call_id.clone(), content));

                let next_index = current_tool_index + 1;
                if next_index < all_tool_calls.len() {
                    // More tool calls to process
                    self.state = StreamState::AwaitingToolDecision {
                        assistant_text,
                        all_tool_calls,
                        current_tool_index: next_index,
                        tool_responses,
                    };
                } else {
                    // All tools processed - build the assistant message with tool calls
                    // Only include text if non-empty
                    let content = if assistant_text.is_empty() {
                        MessageContent::from(all_tool_calls.clone())
                    } else {
                        let mut content = MessageContent::from_text(&assistant_text);
                        for tc in &all_tool_calls {
                            content = content.append(ContentPart::ToolCall(tc.clone()));
                        }
                        content
                    };
                    self.agent.messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        content,
                        options: None,
                    });

                    // Add tool responses
                    for response in tool_responses {
                        self.agent.messages.push(ChatMessage::from(response));
                    }
                    self.state = StreamState::NeedsChatRequest;
                }

                Some(result_step)
            }
            other => {
                // Put state back and return None
                self.state = other;
                None
            }
        }
    }
}
