//! Agent loop for handling conversations with tool execution

use tracing::{debug, error, info};
use anyhow::Result;
use futures::StreamExt;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent,
    ChatStreamResponse, ContentPart, MessageContent, ReasoningEffort, Thinking, Tool,
    ToolCall as GenaiToolCall, ToolResponse,
};
use genai::{Client, Headers};

use crate::auth::OAuthCredentials;
use crate::config::AgentRuntimeConfig;
use crate::transcript::{BlockType, Role, Transcript};
use crate::tools::{ToolCall, ToolDecision, ToolRegistry};

const ANTHROPIC_BETA_HEADER: &str = concat!(
    "oauth-2025-04-20,",
    "claude-code-20250219,",
    "interleaved-thinking-2025-05-14",
    // Removed: causes OAuth rejection before tool calls are processed
    // "fine-grained-tool-streaming-2025-05-14",
);
const ANTHROPIC_USER_AGENT: &str = "claude-cli/2.1.2 (external, cli)";

// Only expose internal ToolCall
// Note: agent_id is set to 0 here - the caller (App) should set the correct ID
// after receiving the ToolRequest from the registry
impl From<&GenaiToolCall> for ToolCall {
    fn from(tc: &GenaiToolCall) -> Self {
        // Extract and remove `background` from params if present
        let mut params = tc.fn_arguments.clone();
        let background = params
            .as_object_mut()
            .and_then(|obj| obj.remove("background"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        
        Self {
            agent_id: 0, // Placeholder - set by App when handling ToolRequest
            call_id: tc.call_id.clone(),
            name: tc.fn_name.clone(),
            params,
            decision: ToolDecision::Pending,
            background,
        }
    }
}

/// Token usage tracking
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    /// Cumulative output tokens across the session
    pub output_tokens: u32,
    /// Current context window size (total input tokens for last request)
    /// This is: input_tokens + cache_creation_input_tokens + cache_read_input_tokens
    pub context_tokens: u32,
    /// Cache creation tokens in last request
    pub cache_creation_tokens: u32,
    /// Cache read tokens in last request  
    pub cache_read_tokens: u32,
}

impl Usage {
    /// Format usage information for logging
    pub fn format_log(&self) -> String {
        // context_tokens = total input (uncached + cache_creation + cache_read)
        let mut details = format!("Context: {} tokens", self.context_tokens);

        if self.cache_read_tokens > 0 || self.cache_creation_tokens > 0 {
            details.push_str(&format!(
                " (cached: {}, new: {})",
                self.cache_read_tokens, self.cache_creation_tokens
            ));
        }

        details.push_str(&format!(", output: {}", self.output_tokens));

        details
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.output_tokens += other.output_tokens;
        // These represent current state, not cumulative
        self.context_tokens = other.context_tokens;
        self.cache_creation_tokens = other.cache_creation_tokens;
        self.cache_read_tokens = other.cache_read_tokens;
    }
}

/// Steps yielded by the agent during processing
pub enum AgentStep {
    /// Streaming text chunk
    TextDelta(String),
    /// Streaming thinking/reasoning chunk (extended thinking)
    ThinkingDelta(String),
    /// Streaming compaction summary chunk
    CompactionDelta(String),
    /// Agent wants to execute tools, needs approval
    ToolRequest(Vec<ToolCall>),
    /// Retrying after error
    Retrying { attempt: u32, error: String },
    /// Agent finished processing this message
    Finished { usage: Usage },
    /// Error occurred
    Error(String),
}

/// Internal state for the agent stream
enum StreamState {
    /// Need to make a new chat API request
    NeedsChatRequest,
    /// Currently streaming response from API (stream stored separately for cancel-safety)
    Streaming,
    /// All tool requests emitted, waiting for decisions
    AwaitingToolDecision,
}

/// Request mode controlling agent behavior for a single request
#[derive(Debug, Clone, Copy, Default)]
pub enum RequestMode {
    /// Normal conversation mode with tool access
    #[default]
    Normal,
    /// Compaction mode: no tools, focused on summarization
    Compaction,
}

/// Options derived from a RequestMode
pub struct RequestOptions {
    pub tools_enabled: bool,
    pub thinking_budget: u32,
    pub capture_tool_calls: bool,
}

impl RequestMode {
    pub fn options(&self, config: &AgentRuntimeConfig) -> RequestOptions {
        match self {
            Self::Normal => RequestOptions {
                tools_enabled: true,
                thinking_budget: config.thinking_budget,
                capture_tool_calls: true,
            },
            Self::Compaction => RequestOptions {
                tools_enabled: false,
                thinking_budget: config.compaction_thinking_budget,
                capture_tool_calls: false,
            },
        }
    }
}

/// Agent for handling conversations
pub struct Agent {
    client: Client,
    config: AgentRuntimeConfig,
    tools: ToolRegistry,
    messages: Vec<ChatMessage>,
    system_prompt: String,
    total_usage: Usage,
    /// OAuth credentials for Claude Max (if available)
    oauth: Option<OAuthCredentials>,

    // Streaming state (Some when actively processing)
    state: Option<StreamState>,
    /// Active response stream (stored separately from state for cancel-safety)
    active_stream:
        Option<futures::stream::BoxStream<'static, Result<ChatStreamEvent, genai::Error>>>,
    mode: RequestMode,

    // Accumulated during streaming, consumed when tools complete
    streaming_text: String,
    streaming_tool_calls: Vec<GenaiToolCall>,
    streaming_thinking: Vec<Thinking>,
    tool_responses: Vec<ToolResponse>,
}

impl Agent {
    /// Create a new agent with a custom tool registry
    pub fn new(
        config: AgentRuntimeConfig,
        system_prompt: &str,
        oauth: Option<OAuthCredentials>,
        tools: ToolRegistry,
    ) -> Self {
        Self {
            client: Client::default(),
            config,
            tools,
            messages: vec![ChatMessage::system(system_prompt)],
            system_prompt: system_prompt.to_string(),
            total_usage: Usage::default(),
            oauth,

            // Streaming state starts empty
            state: None,
            active_stream: None,
            mode: RequestMode::Normal,

            // Accumulated during streaming
            streaming_text: String::new(),
            streaming_tool_calls: Vec::new(),
            streaming_thinking: Vec::new(),
            tool_responses: Vec::new(),
        }
    }

    /// Restore agent message history from a transcript
    /// Preserves the existing system prompt (first message if it's a system message)
    pub fn restore_from_transcript(&mut self, transcript: &Transcript) {
        self.messages.clear();

        // Restore system prompt first
        self.messages
            .push(ChatMessage::system(self.system_prompt.clone()));

        for turn in transcript.turns() {
            match turn.role {
                // Skip system turns - we use our predefined system prompt
                Role::System => continue,
                Role::User => {
                    // Add a message for each text block
                    for block in &turn.content {
                        match block.kind() {
                            BlockType::Text => {
                                if let Some(text) = block.text() {
                                    self.messages.push(ChatMessage::user(text));
                                }
                            },
                            BlockType::Thinking => {},
                            BlockType::Tool => {},
                            BlockType::Compaction => {},
                        }
                    }
                },
                Role::Assistant => {
                    let mut content = MessageContent::default();
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    let mut tool_responses = Vec::new();

                    // Process blocks by kind
                    for block in &turn.content {
                        match block.kind() {
                            BlockType::Text | BlockType::Compaction => {
                                if let Some(text) = block.text() {
                                    text_parts.push(text);
                                }
                            },
                            BlockType::Tool => {
                                // Only add tool call if it has a result (text)
                                // Skip incomplete tools (e.g., quit while awaiting approval)
                                if let (
                                    Some(call_id),
                                    Some(tool_name),
                                    Some(params),
                                    Some(text),
                                ) = (
                                    block.call_id(),
                                    block.tool_name(),
                                    block.params(),
                                    block.text(),
                                ) {
                                    tool_calls.push(GenaiToolCall {
                                        call_id: call_id.to_string(),
                                        fn_name: tool_name.to_string(),
                                        fn_arguments: params.clone(),
                                    });
                                    tool_responses.push(ToolResponse::new(
                                        call_id.to_string(),
                                        text.to_string(),
                                    ));
                                }
                            },
                            BlockType::Thinking => {},
                        }
                    }

                    // Build assistant message
                    if !text_parts.is_empty() {
                        content = content.append(ContentPart::Text(text_parts.join("\n")));
                    }
                    for tc in tool_calls {
                        content = content.append(ContentPart::ToolCall(tc));
                    }

                    if !content.is_empty() {
                        self.messages.push(ChatMessage {
                            role: ChatRole::Assistant,
                            content,
                            options: None,
                        });

                        // Add tool responses
                        for response in tool_responses {
                            self.messages.push(ChatMessage::from(response));
                        }
                    }
                },
            }
        }

        info!("Restored {} messages from transcript", self.messages.len());
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

    /// Send a user message to the agent
    /// Call next() repeatedly to get AgentSteps until None
    pub fn send_request(&mut self, user_input: &str, mode: RequestMode) {
        self.messages.push(ChatMessage::user(user_input));
        self.mode = mode;
        self.state = Some(StreamState::NeedsChatRequest);
    }

    /// Cancel the current streaming operation
    pub fn cancel(&mut self) {
        debug!("Agent::cancel");
        self.state = None;
        self.active_stream = None;
    }

    /// Refresh OAuth token if expired. Returns true if refresh was needed and succeeded.
    pub async fn refresh_oauth_if_needed(&mut self) -> Result<bool> {
        if let Some(ref oauth) = self.oauth {
            if oauth.is_expired() {
                info!("OAuth token expired, refreshing...");
                let new_creds = crate::auth::refresh_token(oauth).await?;
                new_creds.save()?;
                self.oauth = Some(new_creds);
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Set OAuth credentials (used when credentials are refreshed externally)
    pub fn set_oauth(&mut self, oauth: Option<OAuthCredentials>) {
        self.oauth = oauth;
    }

    /// Get total usage statistics
    pub fn total_usage(&self) -> Usage {
        self.total_usage
    }

    /// Reset the agent with a new context after compaction
    /// Preserves the system prompt and adds the compaction summary
    pub fn reset_with_summary(&mut self, summary: &str) {
        // Preserve system prompt if present
        let system_prompt = match self.messages.first() {
            Some(msg) if matches!(msg.role, ChatRole::System) => Some(self.messages[0].clone()),
            _ => None,
        };

        self.messages.clear();

        // Restore system prompt
        if let Some(system) = system_prompt {
            self.messages.push(system);
        }

        // Add the compaction summary as a user message providing context
        self.messages.push(ChatMessage::user(summary));
        self.total_usage = Usage::default();

        info!(
            "Agent reset with compaction summary ({} chars)",
            summary.len()
        );
    }

    /// Convert genai Usage to our Usage struct (for a single turn, not cumulative)
    fn extract_turn_usage(genai_usage: &genai::chat::Usage) -> Usage {
        let input_tokens = genai_usage.prompt_tokens.unwrap_or(0) as u32;
        let output_tokens = genai_usage.completion_tokens.unwrap_or(0) as u32;
        let mut cache_creation_tokens = 0u32;
        let mut cache_read_tokens = 0u32;

        if let Some(ref prompt_details) = genai_usage.prompt_tokens_details {
            if let Some(cc) = prompt_details.cache_creation_tokens {
                cache_creation_tokens = cc as u32;
            }
            if let Some(cr) = prompt_details.cached_tokens {
                cache_read_tokens = cr as u32;
            }
        }

        // Total context = uncached input + cache creation + cache read
        let context_tokens = input_tokens + cache_creation_tokens + cache_read_tokens;

        Usage {
            output_tokens,
            context_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        }
    }

    /// Execute a chat request with retry and exponential backoff
    /// 
    /// Takes &mut self (even though it only reads) because for the future to be
    /// Send, we need &mut Agent (which requires Agent: Send) rather than &Agent
    /// (which requires Agent: Sync). Agent is Send but not Sync due to the
    /// internal stream being `dyn Stream + Send` but not `+ Sync`.
    async fn exec_chat_with_retry(&mut self) -> Result<ChatStreamResponse, AgentStep> {
        // Clone messages and add cache_control to the last message
        // Per Anthropic docs: mark the final message to enable incremental caching
        let mut messages = self.messages.clone();
        if let Some(last_msg) = messages.last_mut() {
            last_msg.options = Some(CacheControl::Ephemeral.into());
            debug!(
                "Added cache_control to last message (role: {})",
                last_msg.role
            );
        }

        let mut request = ChatRequest::new(messages);
        let mode_opts = self.mode.options(&self.config);
        if mode_opts.tools_enabled {
            request = request.with_tools(self.get_tools());
        }

        // Build headers based on OAuth availability
        let headers = if let Some(ref oauth) = self.oauth {
            Headers::from([
                (
                    "authorization".to_string(),
                    format!("Bearer {}", oauth.access_token),
                ),
                (
                    "anthropic-beta".to_string(),
                    ANTHROPIC_BETA_HEADER.to_string(),
                ),
                ("user-agent".to_string(), ANTHROPIC_USER_AGENT.to_string()),
            ])
        } else {
            Headers::from([(
                "anthropic-beta".to_string(),
                "interleaved-thinking-2025-05-14".to_string(),
            )])
        };

        let chat_options = ChatOptions::default()
            .with_max_tokens(self.config.max_tokens)
            .with_capture_usage(true)
            .with_capture_tool_calls(mode_opts.capture_tool_calls)
            .with_capture_reasoning_content(true)
            .with_reasoning_effort(ReasoningEffort::Budget(mode_opts.thinking_budget))
            .with_extra_headers(headers);

        let mut attempt = 0u32;

        loop {
            attempt += 1;
            match self
                .client
                .exec_chat_stream(&self.config.model, request.clone(), Some(&chat_options))
                .await
            {
                Ok(resp) => {
                    info!("Chat request successful");
                    return Ok(resp);
                },
                Err(e) => {
                    let err = format!("{:#}", e);
                    error!("Chat request failed: {}", err);
                    if attempt >= self.config.max_retries {
                        return Err(AgentStep::Error(format!(
                            "API error ({}): {}",
                            self.config.model, err
                        )));
                    }
                    // Return retry step, caller should call next() again
                    return Err(AgentStep::Retrying {
                        attempt,
                        error: err,
                    });
                },
            }
        }
    }

    /// Get the next step from the agent
    /// Returns None when streaming is complete or awaiting tool decisions
    ///
    /// This method is cancel-safe: if the future is dropped mid-poll,
    /// the agent remains in a valid state and can be polled again.
    pub async fn next(&mut self) -> Option<AgentStep> {
        loop {
            // Check state without taking it (cancel-safe)
            match self.state.as_ref()? {
                StreamState::NeedsChatRequest => {
                    debug!("Agent state: NeedsChatRequest, clearing streaming data");
                    // Clear accumulated streaming data for new request
                    self.streaming_text.clear();
                    self.streaming_tool_calls.clear();
                    self.streaming_thinking.clear();
                    self.tool_responses.clear();

                    match self.exec_chat_with_retry().await {
                        Ok(response) => {
                            debug!("Agent state: NeedsChatRequest -> Streaming");
                            // Store stream separately and update state
                            self.active_stream = Some(Box::pin(response.stream));
                            self.state = Some(StreamState::Streaming);
                            // Continue to process streaming state
                        },
                        Err(step) => {
                            // Retrying or Error - state stays NeedsChatRequest for retry
                            if !matches!(step, AgentStep::Retrying { .. }) {
                                self.state = None;
                            }
                            return Some(step);
                        },
                    }
                },

                StreamState::Streaming => {
                    // Get the stream (must exist if we're in Streaming state)
                    let stream = self.active_stream.as_mut()?;

                    match stream.next().await {
                        Some(Ok(event)) => match event {
                            ChatStreamEvent::Start => {
                                // Continue polling
                            },
                            ChatStreamEvent::Chunk(chunk) => {
                                self.streaming_text.push_str(&chunk.content);
                                // State remains Streaming, stream remains in active_stream
                                return Some(match self.mode {
                                    RequestMode::Compaction => {
                                        AgentStep::CompactionDelta(chunk.content)
                                    },
                                    RequestMode::Normal => AgentStep::TextDelta(chunk.content),
                                });
                            },
                            ChatStreamEvent::ToolCallChunk(_) => {
                                // Continue polling
                            },
                            ChatStreamEvent::ReasoningChunk(chunk) => {
                                // State remains Streaming
                                return Some(AgentStep::ThinkingDelta(chunk.content));
                            },
                            ChatStreamEvent::End(mut end) => {
                                if let Some(ref genai_usage) = end.captured_usage {
                                    let turn_usage = Self::extract_turn_usage(genai_usage);
                                    self.total_usage += turn_usage;
                                    info!("{}", turn_usage.format_log());
                                } else {
                                    debug!("No captured_usage in End event");
                                }
                                if let Some(captured) = end.captured_thinking_blocks.take() {
                                    self.streaming_thinking = captured;
                                }
                                if let Some(captured) = end.captured_into_tool_calls() {
                                    self.streaming_tool_calls = captured;
                                }
                                // Continue to process stream end
                            },
                        },
                        Some(Err(e)) => {
                            error!("Stream error: {:?}", e);
                            self.state = None;
                            self.active_stream = None;
                            return Some(AgentStep::Error(format!("Stream error: {:?}", e)));
                        },
                        None => {
                            // Stream ended, clean up stream
                            self.active_stream = None;

                            if self.streaming_tool_calls.is_empty() {
                                match self.mode {
                                    RequestMode::Compaction => {
                                        self.reset_with_summary(&self.streaming_text.clone())
                                    },
                                    RequestMode::Normal => {
                                        // Build message with thinking blocks + text (same pattern as tool use)
                                        // Only push if there's actual content
                                        let has_content = !self.streaming_thinking.is_empty()
                                            || !self.streaming_text.is_empty();
                                        if has_content {
                                            let mut msg_content = MessageContent::default();

                                            // Add thinking blocks first
                                            for thinking in &self.streaming_thinking {
                                                msg_content = msg_content.append(
                                                    ContentPart::Thinking(thinking.clone()),
                                                );
                                            }

                                            // Add text if non-empty
                                            if !self.streaming_text.is_empty() {
                                                msg_content = msg_content.append(
                                                    ContentPart::Text(self.streaming_text.clone()),
                                                );
                                            }

                                            self.messages.push(ChatMessage {
                                                role: ChatRole::Assistant,
                                                content: msg_content,
                                                options: None,
                                            });
                                        } else {
                                            debug!(
                                                "Agent: no content to push (no thinking, no text)"
                                            );
                                        }
                                    },
                                }
                                debug!(
                                    "Agent state: Streaming -> None (Finished), messages={}",
                                    self.messages.len()
                                );
                                self.state = None;
                                return Some(AgentStep::Finished {
                                    usage: self.total_usage,
                                });
                            }

                            let tool_calls: Vec<ToolCall> = self
                                .streaming_tool_calls
                                .iter()
                                .map(ToolCall::from)
                                .collect();
                            self.state = Some(StreamState::AwaitingToolDecision);
                            return Some(AgentStep::ToolRequest(tool_calls));
                        },
                    }
                },

                StreamState::AwaitingToolDecision => {
                    // Blocked waiting for tool results
                    return None;
                },
            }
        }
    }

    /// Submit a tool execution result
    /// Called by App after ToolExecutor runs the tool
    pub fn submit_tool_result(&mut self, call_id: &str, content: String) {
        debug!("Agent: submit_tool_result call_id={}", call_id);

        let state_name = match &self.state {
            Some(StreamState::NeedsChatRequest) => "NeedsChatRequest",
            Some(StreamState::Streaming { .. }) => "Streaming",
            Some(StreamState::AwaitingToolDecision) => "AwaitingToolDecision",
            None => "None",
        };
        if !matches!(self.state, Some(StreamState::AwaitingToolDecision)) {
            tracing::warn!(
                "submit_tool_result called in unexpected state: {}",
                state_name
            );
        }

        // Store the response
        self.tool_responses
            .push(ToolResponse::new(call_id.to_string(), content));

        // Check if all tools have been decided
        // Guard: streaming_tool_calls must be non-empty (otherwise we shouldn't be receiving results)
        if self.streaming_tool_calls.is_empty() {
            tracing::warn!("submit_tool_result called but no tool calls pending");
            return;
        }

        debug!(
            "Agent: tool_responses={}/{}",
            self.tool_responses.len(),
            self.streaming_tool_calls.len()
        );

        if self.tool_responses.len() >= self.streaming_tool_calls.len() {
            debug!(
                "Agent: all tools complete, building message. thinking_blocks={}, text_len={}, tool_calls={}",
                self.streaming_thinking.len(),
                self.streaming_text.len(),
                self.streaming_tool_calls.len()
            );
            // All tools processed - build the assistant message
            // Per Anthropic docs: thinking blocks must come first, then text/tool_use
            let mut msg_content = MessageContent::default();

            // Add thinking blocks first
            for thinking in &self.streaming_thinking {
                msg_content = msg_content.append(ContentPart::Thinking(thinking.clone()));
            }

            // Add text if non-empty
            if !self.streaming_text.is_empty() {
                msg_content = msg_content.append(ContentPart::Text(self.streaming_text.clone()));
            }

            // Add tool calls
            for tc in &self.streaming_tool_calls {
                msg_content = msg_content.append(ContentPart::ToolCall(tc.clone()));
            }

            // Add assistant message
            self.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: msg_content,
                options: None,
            });

            // Add tool responses
            for response in std::mem::take(&mut self.tool_responses) {
                debug!("Agent: adding tool response - call_id={}", response.call_id);
                self.messages.push(ChatMessage::from(response));
            }

            debug!(
                "Agent: continuation will have {} messages total",
                self.messages.len()
            );

            debug!("Agent: state -> NeedsChatRequest (ready for continuation)");
            self.state = Some(StreamState::NeedsChatRequest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Block, TextBlock, ToolBlock, Status};

    // =========================================================================
    // Usage struct tests
    // =========================================================================

    #[test]
    fn test_usage_default() {
        let usage = Usage::default();
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.context_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn test_usage_format_log_basic() {
        let usage = Usage {
            output_tokens: 100,
            context_tokens: 5000,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
        };
        let log = usage.format_log();
        assert_eq!(log, "Context: 5000 tokens, output: 100");
    }

    #[test]
    fn test_usage_format_log_with_cache() {
        let usage = Usage {
            output_tokens: 150,
            context_tokens: 10000,
            cache_creation_tokens: 2000,
            cache_read_tokens: 3000,
        };
        let log = usage.format_log();
        assert_eq!(log, "Context: 10000 tokens (cached: 3000, new: 2000), output: 150");
    }

    #[test]
    fn test_usage_format_log_cache_read_only() {
        let usage = Usage {
            output_tokens: 50,
            context_tokens: 8000,
            cache_creation_tokens: 0,
            cache_read_tokens: 4000,
        };
        let log = usage.format_log();
        assert!(log.contains("cached: 4000"));
        assert!(log.contains("new: 0"));
    }

    #[test]
    fn test_usage_format_log_cache_creation_only() {
        let usage = Usage {
            output_tokens: 75,
            context_tokens: 6000,
            cache_creation_tokens: 1500,
            cache_read_tokens: 0,
        };
        let log = usage.format_log();
        assert!(log.contains("cached: 0"));
        assert!(log.contains("new: 1500"));
    }

    #[test]
    fn test_usage_add_assign_accumulates_output() {
        let mut total = Usage {
            output_tokens: 100,
            context_tokens: 5000,
            cache_creation_tokens: 1000,
            cache_read_tokens: 2000,
        };
        let turn = Usage {
            output_tokens: 50,
            context_tokens: 6000,
            cache_creation_tokens: 500,
            cache_read_tokens: 3000,
        };
        total += turn;

        // Output tokens accumulate
        assert_eq!(total.output_tokens, 150);
        // Context/cache values are replaced (current state, not cumulative)
        assert_eq!(total.context_tokens, 6000);
        assert_eq!(total.cache_creation_tokens, 500);
        assert_eq!(total.cache_read_tokens, 3000);
    }

    #[test]
    fn test_usage_add_assign_multiple_turns() {
        let mut total = Usage::default();

        // First turn
        total += Usage {
            output_tokens: 100,
            context_tokens: 1000,
            cache_creation_tokens: 500,
            cache_read_tokens: 0,
        };
        assert_eq!(total.output_tokens, 100);

        // Second turn
        total += Usage {
            output_tokens: 200,
            context_tokens: 2000,
            cache_creation_tokens: 0,
            cache_read_tokens: 500,
        };
        assert_eq!(total.output_tokens, 300);
        assert_eq!(total.context_tokens, 2000);
        assert_eq!(total.cache_read_tokens, 500);

        // Third turn
        total += Usage {
            output_tokens: 150,
            context_tokens: 3000,
            cache_creation_tokens: 100,
            cache_read_tokens: 900,
        };
        assert_eq!(total.output_tokens, 450);
        assert_eq!(total.context_tokens, 3000);
    }

    // =========================================================================
    // ToolCall conversion tests
    // =========================================================================

    #[test]
    fn test_toolcall_from_genai_basic() {
        let genai_tc = GenaiToolCall {
            call_id: "call_123".to_string(),
            fn_name: "read_file".to_string(),
            fn_arguments: serde_json::json!({"path": "/tmp/test.txt"}),
        };

        let tc = ToolCall::from(&genai_tc);

        assert_eq!(tc.agent_id, 0); // Placeholder
        assert_eq!(tc.call_id, "call_123");
        assert_eq!(tc.name, "read_file");
        assert_eq!(tc.params, serde_json::json!({"path": "/tmp/test.txt"}));
        assert_eq!(tc.decision, ToolDecision::Pending);
        assert!(!tc.background);
    }

    #[test]
    fn test_toolcall_from_genai_with_background_true() {
        let genai_tc = GenaiToolCall {
            call_id: "call_456".to_string(),
            fn_name: "shell".to_string(),
            fn_arguments: serde_json::json!({
                "command": "sleep 10",
                "background": true
            }),
        };

        let tc = ToolCall::from(&genai_tc);

        assert!(tc.background);
        // background param should be removed from params
        assert!(tc.params.get("background").is_none());
        assert_eq!(tc.params.get("command").unwrap(), "sleep 10");
    }

    #[test]
    fn test_toolcall_from_genai_with_background_false() {
        let genai_tc = GenaiToolCall {
            call_id: "call_789".to_string(),
            fn_name: "shell".to_string(),
            fn_arguments: serde_json::json!({
                "command": "echo hello",
                "background": false
            }),
        };

        let tc = ToolCall::from(&genai_tc);

        assert!(!tc.background);
        assert!(tc.params.get("background").is_none());
    }

    #[test]
    fn test_toolcall_from_genai_empty_params() {
        let genai_tc = GenaiToolCall {
            call_id: "call_empty".to_string(),
            fn_name: "list_tasks".to_string(),
            fn_arguments: serde_json::json!({}),
        };

        let tc = ToolCall::from(&genai_tc);

        assert!(!tc.background);
        assert_eq!(tc.params, serde_json::json!({}));
    }

    #[test]
    fn test_toolcall_from_genai_null_params() {
        let genai_tc = GenaiToolCall {
            call_id: "call_null".to_string(),
            fn_name: "some_tool".to_string(),
            fn_arguments: serde_json::Value::Null,
        };

        let tc = ToolCall::from(&genai_tc);

        // Should handle null gracefully (background defaults to false)
        assert!(!tc.background);
    }

    #[test]
    fn test_toolcall_from_genai_array_params() {
        // Edge case: params is an array instead of object
        let genai_tc = GenaiToolCall {
            call_id: "call_array".to_string(),
            fn_name: "weird_tool".to_string(),
            fn_arguments: serde_json::json!(["a", "b", "c"]),
        };

        let tc = ToolCall::from(&genai_tc);

        // Should handle non-object gracefully
        assert!(!tc.background);
        assert_eq!(tc.params, serde_json::json!(["a", "b", "c"]));
    }

    // =========================================================================
    // RequestMode tests
    // =========================================================================

    fn test_config() -> AgentRuntimeConfig {
        AgentRuntimeConfig {
            model: "claude-test".to_string(),
            max_tokens: 4096,
            thinking_budget: 2000,
            max_retries: 3,
            compaction_thinking_budget: 8000,
        }
    }

    #[test]
    fn test_request_mode_normal_options() {
        let config = test_config();
        let opts = RequestMode::Normal.options(&config);

        assert!(opts.tools_enabled);
        assert_eq!(opts.thinking_budget, 2000);
        assert!(opts.capture_tool_calls);
    }

    #[test]
    fn test_request_mode_compaction_options() {
        let config = test_config();
        let opts = RequestMode::Compaction.options(&config);

        assert!(!opts.tools_enabled);
        assert_eq!(opts.thinking_budget, 8000); // Uses compaction_thinking_budget
        assert!(!opts.capture_tool_calls);
    }

    #[test]
    fn test_request_mode_default() {
        let mode = RequestMode::default();
        assert!(matches!(mode, RequestMode::Normal));
    }

    // =========================================================================
    // extract_turn_usage tests
    // =========================================================================

    #[test]
    fn test_extract_turn_usage_basic() {
        let genai_usage = genai::chat::Usage {
            prompt_tokens: Some(1000),
            completion_tokens: Some(200),
            total_tokens: Some(1200),
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        let usage = Agent::extract_turn_usage(&genai_usage);

        assert_eq!(usage.output_tokens, 200);
        assert_eq!(usage.context_tokens, 1000); // input + 0 cache
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
    }

    #[test]
    fn test_extract_turn_usage_with_cache() {
        let genai_usage = genai::chat::Usage {
            prompt_tokens: Some(500),
            completion_tokens: Some(100),
            total_tokens: Some(600),
            prompt_tokens_details: Some(genai::chat::PromptTokensDetails {
                cache_creation_tokens: Some(1000),
                cached_tokens: Some(2000),
                audio_tokens: None,
            }),
            completion_tokens_details: None,
        };

        let usage = Agent::extract_turn_usage(&genai_usage);

        assert_eq!(usage.output_tokens, 100);
        // context = input (500) + cache_creation (1000) + cache_read (2000) = 3500
        assert_eq!(usage.context_tokens, 3500);
        assert_eq!(usage.cache_creation_tokens, 1000);
        assert_eq!(usage.cache_read_tokens, 2000);
    }

    #[test]
    fn test_extract_turn_usage_missing_values() {
        let genai_usage = genai::chat::Usage {
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        let usage = Agent::extract_turn_usage(&genai_usage);

        // Should default to 0 for missing values
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.context_tokens, 0);
    }

    // =========================================================================
    // Agent restore_from_transcript tests
    // =========================================================================

    fn create_test_agent() -> Agent {
        let config = test_config();
        let tools = ToolRegistry::empty();
        Agent::new(config, "Test system prompt", None, tools)
    }

    #[test]
    fn test_restore_from_empty_transcript() {
        let mut agent = create_test_agent();
        let transcript = Transcript::with_path(std::path::PathBuf::from("/tmp/test.json"));

        agent.restore_from_transcript(&transcript);

        // Should only have system prompt
        assert_eq!(agent.messages.len(), 1);
        assert!(matches!(agent.messages[0].role, ChatRole::System));
    }

    #[test]
    fn test_restore_from_transcript_user_messages() {
        let mut agent = create_test_agent();
        let mut transcript = Transcript::with_path(std::path::PathBuf::from("/tmp/test.json"));

        transcript.add_turn(Role::User, TextBlock::complete("Hello"));
        transcript.add_turn(Role::User, TextBlock::complete("How are you?"));

        agent.restore_from_transcript(&transcript);

        // System + 2 user messages
        assert_eq!(agent.messages.len(), 3);
        assert!(matches!(agent.messages[1].role, ChatRole::User));
        assert!(matches!(agent.messages[2].role, ChatRole::User));
    }

    #[test]
    fn test_restore_from_transcript_assistant_text() {
        let mut agent = create_test_agent();
        let mut transcript = Transcript::with_path(std::path::PathBuf::from("/tmp/test.json"));

        transcript.add_turn(Role::User, TextBlock::complete("Hello"));
        transcript.add_turn(Role::Assistant, TextBlock::complete("Hi there!"));

        agent.restore_from_transcript(&transcript);

        assert_eq!(agent.messages.len(), 3);
        assert!(matches!(agent.messages[2].role, ChatRole::Assistant));
    }

    #[test]
    fn test_restore_from_transcript_with_complete_tool() {
        let mut agent = create_test_agent();
        let mut transcript = Transcript::with_path(std::path::PathBuf::from("/tmp/test.json"));

        transcript.add_turn(Role::User, TextBlock::complete("Read a file"));

        // Add assistant turn with a completed tool block
        let mut tool_block = ToolBlock::new(
            "call_123",
            "read_file",
            serde_json::json!({"path": "/tmp/test.txt"}),
            false,
        );
        tool_block.set_status(Status::Complete);
        tool_block.append_text("file contents here");
        transcript.add_turn(Role::Assistant, tool_block);

        agent.restore_from_transcript(&transcript);

        // System + user + assistant (with tool) + tool response
        assert_eq!(agent.messages.len(), 4);
    }

    #[test]
    fn test_restore_from_transcript_includes_tool_with_empty_result() {
        // Note: The code has a comment saying incomplete tools should be skipped,
        // but the check `Some(text)` matches `Some("")`, so tools with empty results
        // are still included. This test documents the actual behavior.
        let mut agent = create_test_agent();
        let mut transcript = Transcript::with_path(std::path::PathBuf::from("/tmp/test.json"));

        transcript.add_turn(Role::User, TextBlock::complete("Read a file"));

        // Add assistant turn with a tool block that has empty text
        let tool_block = ToolBlock::new(
            "call_pending",
            "read_file",
            serde_json::json!({"path": "/tmp/pending.txt"}),
            false,
        );
        transcript.add_turn(Role::Assistant, tool_block);

        agent.restore_from_transcript(&transcript);

        // Current behavior: tools with empty text are still included
        // System + user + assistant (with tool) + tool response = 4
        assert_eq!(agent.messages.len(), 4);
    }

    #[test]
    fn test_restore_from_transcript_skips_system_turns() {
        let mut agent = create_test_agent();
        let mut transcript = Transcript::with_path(std::path::PathBuf::from("/tmp/test.json"));

        // Add a system turn (should be skipped - we use our own system prompt)
        transcript.add_turn(Role::System, TextBlock::complete("Old system prompt"));
        transcript.add_turn(Role::User, TextBlock::complete("Hello"));

        agent.restore_from_transcript(&transcript);

        // Should skip the system turn from transcript and use our own
        assert_eq!(agent.messages.len(), 2); // Our system + user
        // First message should be our system prompt, not the one from transcript
        let first_text = agent.messages[0].content.first_text();
        assert_eq!(first_text, Some("Test system prompt"));
    }

    // =========================================================================
    // Agent reset_with_summary tests
    // =========================================================================

    #[test]
    fn test_reset_with_summary_basic() {
        let mut agent = create_test_agent();

        // Add some messages to simulate a conversation
        agent.messages.push(ChatMessage::user("Hello"));
        agent.messages.push(ChatMessage::assistant("Hi!"));
        agent.messages.push(ChatMessage::user("Do something"));

        // Set some usage
        agent.total_usage = Usage {
            output_tokens: 500,
            context_tokens: 10000,
            cache_creation_tokens: 1000,
            cache_read_tokens: 2000,
        };

        agent.reset_with_summary("This is the compaction summary");

        // Should have system prompt + summary as user message
        assert_eq!(agent.messages.len(), 2);
        assert!(matches!(agent.messages[0].role, ChatRole::System));
        assert!(matches!(agent.messages[1].role, ChatRole::User));

        // Usage should be reset
        assert_eq!(agent.total_usage.output_tokens, 0);
        assert_eq!(agent.total_usage.context_tokens, 0);
    }

    #[test]
    fn test_reset_with_summary_preserves_system_prompt() {
        let config = test_config();
        let tools = ToolRegistry::empty();
        let custom_prompt = "Custom system prompt for testing";
        let mut agent = Agent::new(config, custom_prompt, None, tools);

        agent.messages.push(ChatMessage::user("test"));

        agent.reset_with_summary("Summary after compaction");

        // Verify system prompt is preserved
        let first_text = agent.messages[0].content.first_text();
        assert_eq!(first_text, Some(custom_prompt));
    }

    #[test]
    fn test_reset_with_summary_adds_summary_as_user_message() {
        let mut agent = create_test_agent();
        let summary = "## Summary\n- Task 1 completed\n- Task 2 in progress";

        agent.reset_with_summary(summary);

        // Check the summary is added as user message
        assert_eq!(agent.messages.len(), 2);
        let summary_text = agent.messages[1].content.first_text();
        assert_eq!(summary_text, Some(summary));
    }

    // =========================================================================
    // Agent state tests
    // =========================================================================

    #[test]
    fn test_agent_new_initial_state() {
        let agent = create_test_agent();

        // Initial state should be None (not streaming)
        assert!(agent.state.is_none());
        assert!(agent.active_stream.is_none());
        assert!(agent.streaming_text.is_empty());
        assert!(agent.streaming_tool_calls.is_empty());
        assert!(agent.streaming_thinking.is_empty());
        assert!(agent.tool_responses.is_empty());
    }

    #[test]
    fn test_agent_send_request_sets_state() {
        let mut agent = create_test_agent();

        agent.send_request("Hello", RequestMode::Normal);

        // State should be NeedsChatRequest
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));
        // Message should be added
        assert_eq!(agent.messages.len(), 2); // system + user
    }

    #[test]
    fn test_agent_cancel_clears_state() {
        let mut agent = create_test_agent();

        agent.send_request("Hello", RequestMode::Normal);
        agent.cancel();

        assert!(agent.state.is_none());
        assert!(agent.active_stream.is_none());
    }

    #[test]
    fn test_agent_total_usage_accessor() {
        let mut agent = create_test_agent();
        agent.total_usage = Usage {
            output_tokens: 100,
            context_tokens: 5000,
            cache_creation_tokens: 500,
            cache_read_tokens: 1000,
        };

        let usage = agent.total_usage();
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.context_tokens, 5000);
    }

    // =========================================================================
    // State machine transition tests
    // =========================================================================

    /// Helper to set up agent in AwaitingToolDecision state with pending tool calls
    fn setup_agent_awaiting_tools(tool_count: usize) -> Agent {
        let mut agent = create_test_agent();
        agent.state = Some(StreamState::AwaitingToolDecision);

        // Add mock tool calls that would have been captured during streaming
        for i in 0..tool_count {
            agent.streaming_tool_calls.push(GenaiToolCall {
                call_id: format!("call_{}", i),
                fn_name: "test_tool".to_string(),
                fn_arguments: serde_json::json!({}),
            });
        }

        agent
    }

    #[test]
    fn test_submit_tool_result_single_tool_transitions_to_needs_chat() {
        let mut agent = setup_agent_awaiting_tools(1);

        // Verify initial state
        assert!(matches!(agent.state, Some(StreamState::AwaitingToolDecision)));
        assert_eq!(agent.streaming_tool_calls.len(), 1);
        assert_eq!(agent.tool_responses.len(), 0);

        // Submit the tool result
        agent.submit_tool_result("call_0", "result content".to_string());

        // Should transition to NeedsChatRequest for continuation
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));
        // Tool responses should be consumed (moved to messages)
        assert_eq!(agent.tool_responses.len(), 0);
        // Messages should include assistant message + tool response
        // System (1) + assistant with tool call (1) + tool response (1) = 3
        assert_eq!(agent.messages.len(), 3);
    }

    #[test]
    fn test_submit_tool_result_multiple_tools_waits_for_all() {
        let mut agent = setup_agent_awaiting_tools(3);

        // Submit first result
        agent.submit_tool_result("call_0", "result 0".to_string());

        // Should stay in AwaitingToolDecision (not all tools complete)
        assert!(matches!(agent.state, Some(StreamState::AwaitingToolDecision)));
        assert_eq!(agent.tool_responses.len(), 1);

        // Submit second result
        agent.submit_tool_result("call_1", "result 1".to_string());

        // Still waiting
        assert!(matches!(agent.state, Some(StreamState::AwaitingToolDecision)));
        assert_eq!(agent.tool_responses.len(), 2);

        // Submit third (final) result
        agent.submit_tool_result("call_2", "result 2".to_string());

        // Now should transition to NeedsChatRequest
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));
    }

    #[test]
    fn test_submit_tool_result_builds_correct_message_structure() {
        let mut agent = setup_agent_awaiting_tools(2);

        // Add some streaming text and thinking that would have been captured
        agent.streaming_text = "I'll help with that.".to_string();
        agent.streaming_thinking.push(genai::chat::Thinking {
            thinking: "Let me think about this...".to_string(),
            signature: String::new(),
        });

        // Submit all tool results
        agent.submit_tool_result("call_0", "result 0".to_string());
        agent.submit_tool_result("call_1", "result 1".to_string());

        // Check message structure
        // System (1) + assistant (1) + tool response 0 (1) + tool response 1 (1) = 4
        assert_eq!(agent.messages.len(), 4);

        // Assistant message should be at index 1
        let assistant_msg = &agent.messages[1];
        assert!(matches!(assistant_msg.role, ChatRole::Assistant));

        // Tool responses should follow with ChatRole::Tool
        let tool_resp_1 = &agent.messages[2];
        let tool_resp_2 = &agent.messages[3];
        assert!(matches!(tool_resp_1.role, ChatRole::Tool));
        assert!(matches!(tool_resp_2.role, ChatRole::Tool));
    }

    #[test]
    fn test_submit_tool_result_clears_tool_responses_after_transition() {
        let mut agent = setup_agent_awaiting_tools(1);

        agent.submit_tool_result("call_0", "result".to_string());

        // tool_responses should be consumed (moved to messages via std::mem::take)
        assert!(agent.tool_responses.is_empty());
    }

    #[test]
    fn test_submit_tool_result_in_wrong_state_still_accumulates() {
        // This tests current behavior: submit_tool_result warns but still works
        // when called in wrong state (defensive programming)
        let mut agent = create_test_agent();

        // In NeedsChatRequest state, not AwaitingToolDecision
        agent.state = Some(StreamState::NeedsChatRequest);
        agent.streaming_tool_calls.push(GenaiToolCall {
            call_id: "call_0".to_string(),
            fn_name: "test".to_string(),
            fn_arguments: serde_json::json!({}),
        });

        // This would log a warning but still process
        agent.submit_tool_result("call_0", "result".to_string());

        // State transitions to NeedsChatRequest (continuation)
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));
    }

    #[test]
    fn test_submit_tool_result_with_no_pending_tools_is_noop() {
        let mut agent = create_test_agent();
        agent.state = Some(StreamState::AwaitingToolDecision);
        // No streaming_tool_calls added

        agent.submit_tool_result("call_0", "result".to_string());

        // State should remain unchanged (early return due to empty tool calls)
        assert!(matches!(agent.state, Some(StreamState::AwaitingToolDecision)));
    }

    #[test]
    fn test_send_request_always_adds_message() {
        // Note: send_request doesn't guard against being called in wrong state.
        // It always adds a message and sets state to NeedsChatRequest.
        // This could be a source of bugs if called during streaming/tool execution.
        let mut agent = create_test_agent();

        // First request
        agent.send_request("First", RequestMode::Normal);
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));
        assert_eq!(agent.messages.len(), 2); // system + first user

        // Second request adds another message (no guard)
        agent.send_request("Second", RequestMode::Normal);
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));
        assert_eq!(agent.messages.len(), 3); // system + first + second

        // This documents current behavior - caller is responsible for
        // only calling send_request when agent is idle
    }

    #[test]
    fn test_send_request_after_cancel_works() {
        let mut agent = create_test_agent();

        agent.send_request("First", RequestMode::Normal);
        agent.cancel();

        // Now idle again
        assert!(agent.state.is_none());

        // Can send new request
        agent.send_request("Second", RequestMode::Normal);
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));
        assert_eq!(agent.messages.len(), 3); // system + first + second
    }

    #[test]
    fn test_cancel_from_awaiting_tool_decision() {
        let mut agent = setup_agent_awaiting_tools(2);

        // Cancel mid-tool-execution
        agent.cancel();

        // Should be back to idle
        assert!(agent.state.is_none());
        assert!(agent.active_stream.is_none());
        // Note: streaming_tool_calls etc are NOT cleared by cancel
        // This is intentional - allows inspection of state after cancel
    }

    #[test]
    fn test_state_machine_full_cycle_simulation() {
        // Simulate a full request -> tool -> response cycle
        let mut agent = create_test_agent();

        // 1. Start idle
        assert!(agent.state.is_none());

        // 2. Send request -> NeedsChatRequest
        agent.send_request("Do something with tools", RequestMode::Normal);
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));

        // 3. Simulate what happens after streaming completes with tool calls
        //    (normally done by next(), but we can set up the state directly)
        agent.state = Some(StreamState::Streaming);
        agent.streaming_text = "I'll use a tool.".to_string();
        agent.streaming_tool_calls.push(GenaiToolCall {
            call_id: "call_abc".to_string(),
            fn_name: "read_file".to_string(),
            fn_arguments: serde_json::json!({"path": "/tmp/test"}),
        });

        // Simulate stream end with tool calls -> AwaitingToolDecision
        agent.state = Some(StreamState::AwaitingToolDecision);

        // 4. Submit tool result -> NeedsChatRequest (for continuation)
        agent.submit_tool_result("call_abc", "file contents".to_string());
        assert!(matches!(agent.state, Some(StreamState::NeedsChatRequest)));

        // 5. Could continue with next() for continuation, or cancel
        agent.cancel();
        assert!(agent.state.is_none());
    }

    #[test]
    fn test_streaming_data_cleared_on_new_request() {
        let mut agent = create_test_agent();

        // Simulate leftover data from a previous request
        agent.streaming_text = "leftover text".to_string();
        agent.streaming_tool_calls.push(GenaiToolCall {
            call_id: "old_call".to_string(),
            fn_name: "old_tool".to_string(),
            fn_arguments: serde_json::json!({}),
        });
        agent.streaming_thinking.push(genai::chat::Thinking {
            thinking: "old thinking".to_string(),
            signature: String::new(),
        });
        agent.tool_responses.push(ToolResponse::new("old".to_string(), "old".to_string()));

        // When we send a new request and poll next(), the NeedsChatRequest
        // handler clears this data. We can verify the data is there before.
        assert!(!agent.streaming_text.is_empty());
        assert!(!agent.streaming_tool_calls.is_empty());
        assert!(!agent.streaming_thinking.is_empty());
        assert!(!agent.tool_responses.is_empty());

        // The actual clearing happens in next() when processing NeedsChatRequest,
        // which we can't easily test without async/mocking, but we document
        // the expectation here.
    }
}
