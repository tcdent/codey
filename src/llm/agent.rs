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
                                // Add tool call
                                if let (Some(call_id), Some(tool_name), Some(params)) =
                                    (block.call_id(), block.tool_name(), block.params())
                                {
                                    tool_calls.push(GenaiToolCall {
                                        call_id: call_id.to_string(),
                                        fn_name: tool_name.to_string(),
                                        fn_arguments: params.clone(),
                                    });

                                    // Also collect tool response
                                    if let Some(text) = block.text() {
                                        tool_responses.push(ToolResponse::new(
                                            call_id.to_string(),
                                            text.to_string(),
                                        ));
                                    }
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
