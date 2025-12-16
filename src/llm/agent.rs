//! Agent loop for handling conversations with tool execution

use crate::auth::OAuthCredentials;
use crate::transcript::{Block, Role, Transcript};
use crate::tools::ToolRegistry;
use anyhow::Result;
use futures::StreamExt;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent, ChatStreamResponse,
    ContentPart, MessageContent, ReasoningEffort, Thinking, Tool, ToolCall, ToolResponse,
};
use genai::{Client, Headers};
use std::time::Duration;
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
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
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
        block: Box<dyn Block>,
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
pub struct ModeOptions {
    pub tools_enabled: bool,
    pub thinking_budget: Option<u32>,
    pub capture_tool_calls: bool,
}

impl RequestMode {
    pub fn options(&self) -> ModeOptions {
        match self {
            Self::Normal => ModeOptions {
                tools_enabled: true,
                thinking_budget: None,
                capture_tool_calls: true,
            },
            Self::Compaction => ModeOptions {
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
        messages: Vec<(Role, String)>,
        tools: ToolRegistry,
        oauth: Option<OAuthCredentials>,
    ) -> Self {
        let messages = messages
            .into_iter()
            .map(|(role, text)| match role {
                Role::User => ChatMessage::user(text),
                Role::Assistant => ChatMessage::assistant(text),
                // Cache system messages for reuse across requests
                Role::System => ChatMessage::system(text).with_options(CacheControl::Ephemeral),
            })
            .collect();

        Self {
            client: Client::default(),
            model: model.into(),
            max_tokens,
            max_retries,
            tools,
            messages,
            total_usage: Usage::default(),
            oauth,
        }
    }

    /// Restore agent message history from a transcript
    /// Preserves the existing system prompt (first message if it's a system message)
    pub fn restore_from_transcript(&mut self, transcript: &Transcript) {
        // Preserve system prompt if present (should be first message)
        // TODO we're not going to serialize system prompts in transcripts
        let system_prompt = match self.messages.first() {
            Some(msg) if matches!(msg.role, ChatRole::System) => Some(self.messages.remove(0)),
            _ => None,
        };
        
        self.messages.clear();
        
        // Restore system prompt first
        if let Some(system) = system_prompt {
            self.messages.push(system);
        }
        
        for turn in transcript.turns() {
            // Only restore completed turns - skip incomplete/running/cancelled turns
            // This also filters out thinking blocks since they don't have `status`
            if turn.status != crate::transcript::Status::Complete {
                continue;
            }
            
            match turn.role {
                // Skip system turns - we use our predefined system prompt
                Role::System => continue,
                Role::User => {
                    // For user turns, just collect text
                    let text: String = turn.content
                        .iter()
                        .filter_map(|block| block.text_content())
                        .collect::<Vec<_>>()
                        .join("\n");
                    
                    if !text.is_empty() {
                        // Don't add cache_control here - we'll add it to the last message only
                        self.messages.push(ChatMessage::user(text));
                    }
                }
                Role::Assistant => {
                    // Build message content: thinking first, then text, then tool calls
                    let mut content = MessageContent::default();
                    
                    // Process blocks in order, but thinking blocks must come first
                    // First pass: add thinking blocks
                    for block in &turn.content {
                        if let (Some(text), Some(sig)) = (block.text_content(), block.signature()) {
                            content = content.append(ContentPart::Thinking(Thinking::new(text, sig)));
                        }
                    }
                    
                    // Second pass: add text blocks (those without signatures)
                    let text: String = turn.content
                        .iter()
                        .filter(|block| block.signature().is_none())
                        .filter_map(|block| block.text_content())
                        .collect::<Vec<_>>()
                        .join("\n");
                    
                    if !text.is_empty() {
                        content = content.append(ContentPart::Text(text));
                    }
                    
                    // Third pass: add tool calls
                    for block in &turn.content {
                        if let (Some(call_id), Some(tool_name), Some(params)) = 
                            (block.call_id(), block.tool_name(), block.params()) 
                        {
                            let tc = ToolCall {
                                call_id: call_id.to_string(),
                                fn_name: tool_name.to_string(),
                                fn_arguments: params.clone(),
                            };
                            content = content.append(ContentPart::ToolCall(tc));
                        }
                    }
                    
                    if !content.is_empty() {
                        // Don't add cache_control here - we'll add it to the last message only
                        self.messages.push(ChatMessage {
                            role: ChatRole::Assistant,
                            content,
                            options: None,
                        });
                        
                        // Add tool responses without cache_control - we'll add it to the last message only
                        for block in &turn.content {
                            if let (Some(call_id), Some(result)) = (block.call_id(), block.result()) {
                                let response = ToolResponse::new(call_id.to_string(), result.to_string());
                                self.messages.push(ChatMessage::from(response));
                            }
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
        full_thinking: String,
        tool_calls: Vec<ToolCall>,
        thinking_blocks: Vec<Thinking>,
    },
    /// Waiting for tool approval decision
    AwaitingToolDecision {
        assistant_text: String,
        all_tool_calls: Vec<ToolCall>,
        thinking_blocks: Vec<Thinking>, // TODO we may not need to actually capture these. 
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
                                full_thinking: String::new(),
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
                    full_thinking,
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
                                    full_thinking.push_str(&chunk.content);
                                    return Some(AgentStep::ThinkingDelta(chunk.content));
                                }
                                ChatStreamEvent::End(mut end) => {
                                    if let Some(ref usage) = end.captured_usage {
                                        let input = usage.prompt_tokens.unwrap_or(0) as u32;
                                        let output = usage.completion_tokens.unwrap_or(0) as u32;

                                        self.agent.total_usage.input_tokens += input;
                                        self.agent.total_usage.output_tokens += output;
                                        // Track current context size for compaction decisions
                                        self.agent.total_usage.context_tokens = input;

                                        // Log token usage details
                                        let mut details = format!("Turn tokens: input={} output={}", input, output);
                                        
                                        if let Some(ref prompt_details) = usage.prompt_tokens_details {
                                            if let Some(cache_creation) = prompt_details.cache_creation_tokens {
                                                details.push_str(&format!(" cache_creation={}", cache_creation));
                                            }
                                            if let Some(cached) = prompt_details.cached_tokens {
                                                details.push_str(&format!(" cached={}", cached));
                                            }
                                        }
                                        
                                        if let Some(ref completion_details) = usage.completion_tokens_details {
                                            if let Some(reasoning) = completion_details.reasoning_tokens {
                                                details.push_str(&format!(" reasoning={}", reasoning));
                                            }
                                        }
                                        
                                        info!("{}", details);
                                        
                                        // Debug: log full usage details
                                        debug!("Full usage: prompt_tokens={:?}, completion_tokens={:?}, total_tokens={:?}",
                                            usage.prompt_tokens, usage.completion_tokens, usage.total_tokens);
                                        debug!("Prompt details: {:?}", usage.prompt_tokens_details);
                                        debug!("Completion details: {:?}", usage.completion_tokens_details);
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
                    let _full_thinking = std::mem::take(full_thinking);
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
