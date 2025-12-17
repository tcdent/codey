//! Agent loop for handling conversations with tool execution

use crate::auth::OAuthCredentials;
use crate::transcript::{BlockType, Role, Transcript};
use crate::tools::ToolRegistry;
use anyhow::Result;
use futures::StreamExt;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent, ChatStreamResponse,
    ContentPart, MessageContent, ReasoningEffort, Thinking, Tool, ToolCall, ToolResponse,
};
use genai::{Client, Headers};
use tracing::{debug, error, info};

const ANTHROPIC_BETA_HEADER: &str = concat!(
    "oauth-2025-04-20,",
    "claude-code-20250219,",
    "interleaved-thinking-2025-05-14,",
    "fine-grained-tool-streaming-2025-05-14",
);
const ANTHROPIC_USER_AGENT: &str = "ai-sdk/anthropic/2.0.50 ai-sdk/provider-utils/3.0.18 runtime/bun/1.3.4";

/// Token usage tracking
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    /// Cumulative input tokens across the session
    pub input_tokens: u32,
    /// Cumulative output tokens across the session
    pub output_tokens: u32,
    /// Current context window size (last request's input tokens)
    /// This is used to determine when compaction is needed
    pub context_tokens: u32,
    /// Cumulative cache creation tokens (tokens written to cache)
    pub cache_creation_tokens: u32,
    /// Cumulative cache read tokens (tokens read from cache)
    pub cache_read_tokens: u32,
}

impl Usage {
    /// Format usage information for logging
    pub fn format_log(&self, genai_usage: &genai::chat::Usage) -> String {
        let mut details = format!("Turn tokens: input={} output={}", self.input_tokens, self.output_tokens);
        
        if self.cache_creation_tokens > 0 {
            details.push_str(&format!(" cache_creation={}", self.cache_creation_tokens));
        }
        if self.cache_read_tokens > 0 {
            details.push_str(&format!(" cached={}", self.cache_read_tokens));
        }
        
        if let Some(ref completion_details) = genai_usage.completion_tokens_details {
            if let Some(reasoning) = completion_details.reasoning_tokens {
                details.push_str(&format!(" reasoning={}", reasoning));
            }
        }
        
        details
    }
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        // context_tokens is set directly, not accumulated
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
    /// Agent wants to execute a tool, needs approval
    ToolRequest {
        call_id: String,
        name: String,
        params: serde_json::Value,
    },
    /// Tool finished executing
    ToolResult {
        call_id: String,
        result: String,
        is_error: bool,
    },
    /// Retrying after error
    Retrying {
        attempt: u32,
        error: String,
    },
    /// Agent finished processing this message
    Finished {
        usage: Usage,
        /// Signatures for thinking blocks (in order they appeared)
        thinking_signatures: Vec<String>,
    },
    /// Error occurred
    Error(String),
}

pub use crate::permission::ToolDecision;

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
    pub thinking_budget: Option<u32>,
    pub capture_tool_calls: bool,
}

impl RequestMode {
    pub fn options(&self) -> RequestOptions {
        match self {
            Self::Normal => RequestOptions {
                tools_enabled: true,
                thinking_budget: None,
                capture_tool_calls: true,
            },
            Self::Compaction => RequestOptions {
                tools_enabled: false,
                thinking_budget: Some(8000),
                capture_tool_calls: false,
            },
        }
    }
}

/// Agent for handling conversations
pub struct Agent {
    client: Client,
    model: String,
    max_tokens: u32,
    max_retries: u32,
    tools: ToolRegistry,
    messages: Vec<ChatMessage>,
    system_prompt: String,
    total_usage: Usage,
    /// OAuth credentials for Claude Max (if available)
    oauth: Option<OAuthCredentials>,
}

impl Agent {
    /// Create a new agent with initial messages
    pub fn new(
        model: impl Into<String>,
        max_tokens: u32,
        max_retries: u32,
        system_prompt: &str,
        tools: ToolRegistry,
        oauth: Option<OAuthCredentials>,
    ) -> Self {
        Self {
            client: Client::default(),
            model: model.into(),
            max_tokens,
            max_retries,
            tools,
            messages: vec![ChatMessage::system(system_prompt)],
            system_prompt: system_prompt.to_string(),
            total_usage: Usage::default(),
            oauth,
        }
    }

    /// Restore agent message history from a transcript
    /// Preserves the existing system prompt (first message if it's a system message)
    pub fn restore_from_transcript(&mut self, transcript: &Transcript) {
        self.messages.clear();
        
        // Restore system prompt first
        self.messages.push(ChatMessage::system(self.system_prompt.clone()));
        
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
                            }
                            _  => {}
                        }
                    }
                }
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
                            }
                            BlockType::Tool => {
                                // Add tool call
                                if let (Some(call_id), Some(tool_name), Some(params)) = 
                                    (block.call_id(), block.tool_name(), block.params()) 
                                {
                                    tool_calls.push(ToolCall {
                                        call_id: call_id.to_string(),
                                        fn_name: tool_name.to_string(),
                                        fn_arguments: params.clone(),
                                    });
                                    
                                    // Also collect tool response
                                    if let Some(text) = block.text() {
                                        tool_responses.push(ToolResponse::new(call_id.to_string(), text.to_string()));
                                    }
                                }
                            }
                            BlockType::Thinking => {
                                // Skip - thinking is not restored to API
                            }
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
                }
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

    /// Start processing a user message with a specific mode
    pub fn process_message(&mut self, user_input: &str, mode: RequestMode) -> AgentStream<'_> {
        // Add user message (cache_control will be applied dynamically before API call)
        self.messages.push(ChatMessage::user(user_input));
        AgentStream::new(self, mode)
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

    /// Get current context size in tokens
    pub fn context_tokens(&self) -> u32 {
        self.total_usage.context_tokens
    }

    /// Get a tool by name
    pub fn get_tool(&self, name: &str) -> &dyn crate::tools::Tool {
        self.tools.get(name)
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

        // Clear all messages
        self.messages.clear();

        // Restore system prompt
        if let Some(system) = system_prompt {
            self.messages.push(system);
        }

        // Add the compaction summary as a user message providing context
        self.messages.push(ChatMessage::user(summary));

        // Reset usage tracking (but keep context_tokens as reference)
        self.total_usage = Usage::default();

        info!("Agent reset with compaction summary ({} chars)", summary.len());
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
        thinking_blocks: Vec<Thinking>,
    },
    /// Waiting for tool approval decision
    AwaitingToolDecision {
        assistant_text: String,
        all_tool_calls: Vec<ToolCall>,
        thinking_blocks: Vec<Thinking>, 
        current_tool_index: usize,
        tool_responses: Vec<ToolResponse>,
    },
    /// Processing completed
    Finished,
}

/// Stream that yields AgentSteps and accepts ToolDecisions
pub struct AgentStream<'a> {
    pub agent: &'a mut Agent,
    state: StreamState,
    tools: Vec<Tool>,
    chat_options: ChatOptions,
    /// Accumulated thinking signatures across all rounds of the agent loop
    accumulated_signatures: Vec<String>,
    /// Request mode for this stream
    mode: RequestMode,
}

/// Default thinking budget in tokens (16k allows substantial reasoning)
const DEFAULT_THINKING_BUDGET: u32 = 16000;

impl<'a> AgentStream<'a> {
    fn new(agent: &'a mut Agent, mode: RequestMode) -> Self {
        let opts = mode.options();
        let tools = if opts.tools_enabled { agent.get_tools() } else { Vec::new() };

        // Build headers based on OAuth availability
        let headers = if let Some(ref oauth) = agent.oauth {
            // OAuth mode: use Bearer auth with required beta headers (no x-api-key)
            // Match OpenCode's exact header format
            Headers::from([
                ("authorization".to_string(), format!("Bearer {}", oauth.access_token)),
                ("anthropic-beta".to_string(), ANTHROPIC_BETA_HEADER.to_string()),
                ("user-agent".to_string(), ANTHROPIC_USER_AGENT.to_string()),
            ])
        } else {
            // API key mode: just the thinking beta header
            Headers::from([(
                "anthropic-beta".to_string(),
                "interleaved-thinking-2025-05-14".to_string(),
            )])
        };

        // Apply mode settings with agent defaults as fallback
        let thinking_budget = opts.thinking_budget.unwrap_or(DEFAULT_THINKING_BUDGET);

        let chat_options = ChatOptions::default()
            .with_max_tokens(agent.max_tokens)
            .with_capture_usage(true)
            .with_capture_tool_calls(opts.capture_tool_calls)
            .with_capture_reasoning_content(true)
            .with_reasoning_effort(ReasoningEffort::Budget(thinking_budget))
            .with_extra_headers(headers);

        Self {
            agent,
            state: StreamState::NeedsChatRequest,
            tools,
            chat_options,
            accumulated_signatures: Vec::new(),
            mode,
        }
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

        Usage {
            input_tokens,
            output_tokens,
            context_tokens: input_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        }
    }

    /// Execute a chat request with retry and exponential backoff
    async fn exec_chat_with_retry(&self) -> Result<ChatStreamResponse, AgentStep> {
        // Clone messages and add cache_control to the last message
        // Per Anthropic docs: mark the final message to enable incremental caching
        let mut messages = self.agent.messages.clone();
        if let Some(last_msg) = messages.last_mut() {
            last_msg.options = Some(CacheControl::Ephemeral.into());
            debug!("Added cache_control to last message (role: {})", last_msg.role);
        }
        
        // Count messages with cache_control
        let cache_control_count = messages.iter().filter(|m| m.options.is_some()).count();
        debug!("Messages with cache_control: {}/{}", cache_control_count, messages.len());
        
        // Debug: log the last message role to verify
        if let Some(last) = messages.last() {
            debug!("Last message role: {}, has_cache_control: {}", last.role, last.options.is_some());
        }
        
        let mut request = ChatRequest::new(messages);
        if !self.tools.is_empty() {
            request = request.with_tools(self.tools.clone());
        }

        info!(
            "Making chat request with {} messages",
            self.agent.messages.len()
        );

        let mut attempt = 0u32;

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
                                thinking_blocks: Vec::new(),
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
                    thinking_blocks,
                } => {
                    while let Some(result) = stream.next().await {
                        match result {
                            Ok(event) => match event {
                                ChatStreamEvent::Start => {}
                                ChatStreamEvent::Chunk(chunk) => {
                                    full_text.push_str(&chunk.content);
                                    return Some(match self.mode {
                                        RequestMode::Compaction => AgentStep::CompactionDelta(chunk.content),
                                        RequestMode::Normal => AgentStep::TextDelta(chunk.content),
                                    });
                                }
                                ChatStreamEvent::ToolCallChunk(_) => {}
                                ChatStreamEvent::ReasoningChunk(chunk) => {
                                    return Some(AgentStep::ThinkingDelta(chunk.content));
                                }
                                ChatStreamEvent::End(mut end) => {
                                    if let Some(ref genai_usage) = end.captured_usage {
                                        let turn_usage = Self::extract_turn_usage(genai_usage);
                                        self.agent.total_usage += turn_usage;
                                        info!("{}", turn_usage.format_log(genai_usage));
                                    }
                                    // Capture thinking blocks first (before consuming end)
                                    if let Some(captured) = end.captured_thinking_blocks.take() {
                                        *thinking_blocks = captured;
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
                    let thinking_blocks = std::mem::take(thinking_blocks);

                    // Accumulate signatures from this round's thinking blocks
                    for thinking in thinking_blocks.iter() {
                        self.accumulated_signatures.push(thinking.signature.clone());
                    }

                    if tool_calls.is_empty() {
                        match self.mode {
                            RequestMode::Compaction => self.agent.reset_with_summary(&full_text),
                            RequestMode::Normal => self.agent.messages.push(ChatMessage::assistant(&full_text)),
                        }
                        let signatures = std::mem::take(&mut self.accumulated_signatures);
                        self.state = StreamState::Finished;
                        return Some(AgentStep::Finished {
                            usage: self.agent.total_usage,
                            thinking_signatures: signatures,
                        });
                    }

                    // Start processing tool calls (don't add message yet - wait until all done)
                    self.state = StreamState::AwaitingToolDecision {
                        assistant_text: full_text,
                        all_tool_calls: tool_calls,
                        thinking_blocks,
                        current_tool_index: 0,
                        tool_responses: Vec::new(),
                    };
                    // Continue to process tool decision state
                }

                StreamState::AwaitingToolDecision {
                    ref all_tool_calls,
                    current_tool_index,
                    ..
                } => {
                    // Yield the current tool request
                    let tool_call = &all_tool_calls[*current_tool_index];

                    return Some(AgentStep::ToolRequest {
                        call_id: tool_call.call_id.clone(),
                        name: tool_call.fn_name.clone(),
                        params: tool_call.fn_arguments.clone(),
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
                thinking_blocks,
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
                        thinking_blocks,
                        current_tool_index: next_index,
                        tool_responses,
                    };
                } else {
                    // All tools processed - build the assistant message with thinking + text + tool calls
                    // Per Anthropic docs: thinking blocks must come first, then text/tool_use
                    let mut content = MessageContent::default();
                    
                    // Add thinking blocks first (required for extended thinking with tool use)
                    for thinking in &thinking_blocks {
                        content = content.append(ContentPart::Thinking(thinking.clone()));
                    }
                    
                    // Add text if non-empty
                    if !assistant_text.is_empty() {
                        content = content.append(ContentPart::Text(assistant_text));
                    }
                    
                    // Add tool calls
                    for tc in &all_tool_calls {
                        content = content.append(ContentPart::ToolCall(tc.clone()));
                    }
                    
                    // Add assistant message without cache control
                    self.agent.messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        content,
                        options: None,
                    });

                    // Add tool responses without cache control
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
