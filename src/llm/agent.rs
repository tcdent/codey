//! Agent loop for handling conversations with tool execution

use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use anyhow::Result;
use futures::StreamExt;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatRole, ChatStreamEvent,
    ChatStreamResponse, ContentPart, MessageContent, ReasoningEffort, Thinking, Tool,
    ToolCall as GenaiToolCall, ToolResponse,
};
use genai::{Client, Headers};

use super::client::build_client;
use super::client::is_openrouter_model;

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
const ANTHROPIC_USER_AGENT: &str = "claude-code/2.1.37 (external, cli)";

/// Beta header value that activates fast mode (research preview).
const FAST_MODE_BETA: &str = "research-preview-2026-02-01";

/// Duration to cool down fast mode after a rate limit, before re-enabling.
const FAST_MODE_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(20 * 60);

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

/// Internal state for the agent stream.
///
/// State-specific data is carried inside each variant, enforcing at compile time
/// that fields like the active stream or pending tool responses can only be
/// accessed when the agent is in the corresponding state.
enum StreamState {
    /// Need to make a new chat API request
    NeedsChatRequest,
    /// Currently streaming response from API
    Streaming {
        stream: futures::stream::BoxStream<'static, Result<ChatStreamEvent, genai::Error>>,
    },
    /// All tool requests emitted, waiting for decisions
    AwaitingToolDecision {
        /// The tool calls the model requested (used for count-checking and message building)
        pending_tool_calls: Vec<GenaiToolCall>,
        /// Tool results submitted so far
        tool_responses: Vec<ToolResponse>,
    },
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

/// A function that builds a dynamic system prompt.
/// Called before each LLM request to allow prompt content to change.
pub type SystemPromptBuilder = Box<dyn Fn() -> String + Send + Sync>;

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
    /// Optional dynamic prompt builder - called before each request
    system_prompt_builder: Option<SystemPromptBuilder>,

    // Streaming state (Some when actively processing)
    state: Option<StreamState>,
    mode: RequestMode,

    // Accumulated during streaming, consumed when tools complete or stream ends.
    // These span multiple states (built during Streaming, read during AwaitingToolDecision
    // completion), so they live on Agent rather than inside a single variant.
    streaming_text: String,
    streaming_thinking: Vec<Thinking>,

    /// When set, fast mode is cooling down until this instant.
    /// During cooldown, the fast mode beta header is omitted from requests.
    fast_mode_cooldown_until: Option<Instant>,

    /// Retry attempt counter, persists across calls to exec_chat_with_retry.
    /// Reset on successful request or new user message.
    retry_attempt: u32,
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
            client: build_client(),
            config,
            tools,
            messages: vec![ChatMessage::system(system_prompt)],
            system_prompt: system_prompt.to_string(),
            total_usage: Usage::default(),
            oauth,
            system_prompt_builder: None,

            // Streaming state starts empty
            state: None,
            mode: RequestMode::Normal,

            // Accumulated during streaming
            streaming_text: String::new(),
            streaming_thinking: Vec::new(),

            fast_mode_cooldown_until: None,
            retry_attempt: 0,
        }
    }

    /// Create a new agent with a dynamic system prompt builder.
    ///
    /// The builder is called before each LLM request, allowing the prompt
    /// to include dynamic content (e.g., mdsh-processed shell command output).
    pub fn with_dynamic_prompt(
        config: AgentRuntimeConfig,
        prompt_builder: SystemPromptBuilder,
        oauth: Option<OAuthCredentials>,
        tools: ToolRegistry,
    ) -> Self {
        let system_prompt = prompt_builder();
        Self {
            client: build_client(),
            config,
            tools,
            messages: vec![ChatMessage::system(&system_prompt)],
            system_prompt,
            total_usage: Usage::default(),
            oauth,
            system_prompt_builder: Some(prompt_builder),

            // Streaming state starts empty
            state: None,
            mode: RequestMode::Normal,

            // Accumulated during streaming
            streaming_text: String::new(),
            streaming_thinking: Vec::new(),

            fast_mode_cooldown_until: None,
            retry_attempt: 0,
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
                                        thought_signatures: None,
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
        self.retry_attempt = 0;
        self.state = Some(StreamState::NeedsChatRequest);
    }

    /// Cancel the current streaming operation.
    /// Dropping the state also drops any active stream or pending tool data.
    pub fn cancel(&mut self) {
        debug!("Agent::cancel");
        self.state = None;
    }

    /// Refresh OAuth token if expired. Returns true if refresh was needed and succeeded.
    #[allow(dead_code)]
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

    /// Refresh the system prompt if a dynamic builder is configured.
    ///
    /// This is called before each LLM request to allow the prompt content
    /// to change (e.g., when mdsh commands return different output).
    fn refresh_system_prompt(&mut self) {
        if let Some(ref builder) = self.system_prompt_builder {
            let new_prompt = builder();
            if new_prompt != self.system_prompt {
                debug!("System prompt changed:\n{}", new_prompt);
                self.system_prompt = new_prompt.clone();
                // Update the first message (system message)
                if !self.messages.is_empty() {
                    self.messages[0] = ChatMessage::system(&new_prompt);
                }
            }
        }
    }

    /// Get total usage statistics
    pub fn total_usage(&self) -> Usage {
        self.total_usage
    }

    /// Get the last assistant message text (for returning sub-agent results).
    /// Returns the accumulated streaming text if present.
    pub fn last_message(&self) -> Option<String> {
        if self.streaming_text.is_empty() {
            None
        } else {
            Some(self.streaming_text.clone())
        }
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

    /// Returns true if fast mode is currently active.
    ///
    /// Fast mode requires: config flag enabled, model is opus-4-6, and not in cooldown.
    pub fn is_fast_mode(&self) -> bool {
        if !self.config.fast_mode {
            return false;
        }
        if !self.config.model.to_lowercase().contains("opus-4-6") {
            return false;
        }
        if let Some(until) = self.fast_mode_cooldown_until {
            if Instant::now() < until {
                return false;
            }
        }
        true
    }

    /// Icon to prepend to the model name in the UI.
    pub fn model_icon(&self) -> &'static str {
        if self.is_fast_mode() {
            "ϟ"
        } else {
            ""
        }
    }

    /// Check if an error message indicates a rate limit (429) or overloaded (529) response.
    fn is_rate_limit_error(&self, error: &str) -> bool {
        let lower = error.to_lowercase();
        lower.contains("429")
            || lower.contains("rate limit")
            || lower.contains("overloaded")
            || lower.contains("529")
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

        // Check fast mode status before building headers
        let fast_mode_active = self.is_fast_mode();
        if fast_mode_active {
            info!("Fast mode active for this request");
        }

        // Build headers based on provider and OAuth availability
        let headers = if is_openrouter_model(&self.config.model) {
            // OpenRouter uses standard Bearer auth (handled by client resolver)
            // Add recommended headers for app attribution
            Headers::from([
                ("HTTP-Referer".to_string(), "https://github.com/tcdent/codey".to_string()),
                ("X-Title".to_string(), "Codey".to_string()),
            ])
        } else if let Some(ref oauth) = self.oauth {
            let mut beta = ANTHROPIC_BETA_HEADER.to_string();
            if fast_mode_active {
                beta.push(',');
                beta.push_str(FAST_MODE_BETA);
            }
            Headers::from([
                (
                    "authorization".to_string(),
                    format!("Bearer {}", oauth.access_token),
                ),
                ("anthropic-beta".to_string(), beta),
                ("user-agent".to_string(), ANTHROPIC_USER_AGENT.to_string()),
            ])
        } else {
            let mut beta = "interleaved-thinking-2025-05-14".to_string();
            if fast_mode_active {
                beta.push(',');
                beta.push_str(FAST_MODE_BETA);
            }
            Headers::from([("anthropic-beta".to_string(), beta)])
        };

        // Build chat options - reasoning_effort is only for Anthropic models
        let mut chat_options = ChatOptions::default()
            .with_max_tokens(self.config.max_tokens)
            .with_capture_usage(true)
            .with_capture_tool_calls(mode_opts.capture_tool_calls)
            .with_extra_headers(headers);
        
        // Only add reasoning/thinking options for Anthropic models
        if !is_openrouter_model(&self.config.model) {
            chat_options = chat_options
                .with_capture_reasoning_content(true)
                .with_reasoning_effort(ReasoningEffort::Budget(mode_opts.thinking_budget));
        }

        self.retry_attempt += 1;
        match self
            .client
            .exec_chat_stream(&self.config.model, request.clone(), Some(&chat_options))
            .await
        {
            Ok(resp) => {
                info!("Chat request successful");
                self.retry_attempt = 0;
                Ok(resp)
            },
            Err(e) => {
                let err = format!("{:#}", e);
                error!("Chat request failed (attempt {}): {}", self.retry_attempt, err);

                // If fast mode is active and we hit a rate limit or overloaded
                // error, trigger cooldown and retry without the fast mode header.
                if fast_mode_active && self.is_rate_limit_error(&err) {
                    warn!(
                        "Fast mode rate limited, entering {}s cooldown",
                        FAST_MODE_COOLDOWN.as_secs()
                    );
                    self.fast_mode_cooldown_until = Some(Instant::now() + FAST_MODE_COOLDOWN);
                    // Don't count fast mode fallback as a retry attempt
                    self.retry_attempt -= 1;
                    return Err(AgentStep::Retrying {
                        attempt: self.retry_attempt,
                        error: format!("Fast mode rate limited, falling back to standard speed"),
                    });
                }

                if self.retry_attempt >= self.config.max_retries {
                    self.retry_attempt = 0;
                    return Err(AgentStep::Error(format!(
                        "API error ({}): {}",
                        self.config.model, err
                    )));
                }
                // Return retry step, caller should call next() again
                Err(AgentStep::Retrying {
                    attempt: self.retry_attempt,
                    error: err,
                })
            },
        }
    }

    /// Get the next step from the agent
    /// Returns None when streaming is complete or awaiting tool decisions
    ///
    /// This method is cancel-safe: if the future is dropped mid-poll,
    /// the agent remains in a valid state and can be polled again.
    pub async fn next(&mut self) -> Option<AgentStep> {
        loop {
            match self.state.as_mut()? {
                StreamState::NeedsChatRequest => {
                    debug!("Agent state: NeedsChatRequest, clearing streaming data");

                    // Exponential backoff before retrying: 2s, 4s, 8s, 16s, ...
                    if self.retry_attempt > 0 {
                        let delay = Duration::from_secs(2u64.pow(self.retry_attempt));
                        info!("Backoff: waiting {}s before retry attempt {}", delay.as_secs(), self.retry_attempt + 1);
                        tokio::time::sleep(delay).await;
                    }

                    // Refresh dynamic system prompt before each request
                    self.refresh_system_prompt();

                    // Clear accumulated cross-state data for new request
                    self.streaming_text.clear();
                    self.streaming_thinking.clear();

                    match self.exec_chat_with_retry().await {
                        Ok(response) => {
                            debug!("Agent state: NeedsChatRequest -> Streaming");
                            self.state = Some(StreamState::Streaming {
                                stream: Box::pin(response.stream),
                            });
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

                StreamState::Streaming { stream } => {
                    match stream.next().await {
                        Some(Ok(event)) => match event {
                            ChatStreamEvent::Start => {
                                debug!("Agent: got ChatStreamEvent::Start");
                                // Continue polling
                            },
                            ChatStreamEvent::Chunk(chunk) => {
                                self.streaming_text.push_str(&chunk.content);
                                return Some(match self.mode {
                                    RequestMode::Compaction => {
                                        AgentStep::CompactionDelta(chunk.content)
                                    },
                                    RequestMode::Normal => AgentStep::TextDelta(chunk.content),
                                });
                            },
                            ChatStreamEvent::ToolCallChunk(_) => {
                                debug!("Agent: got ToolCallChunk");
                                // Continue polling
                            },
                            ChatStreamEvent::ReasoningChunk(chunk) => {
                                return Some(AgentStep::ThinkingDelta(chunk.content));
                            },
                            ChatStreamEvent::ThoughtSignatureChunk(_) => {
                                // Gemini thought signatures - continue polling
                            },
                            ChatStreamEvent::End(mut end) => {
                                debug!("Agent: got ChatStreamEvent::End");
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
                                // Capture tool calls from the End event; used below
                                // when the stream closes (None branch).
                                let streaming_tool_calls = end
                                    .captured_into_tool_calls()
                                    .unwrap_or_default();

                                if !streaming_tool_calls.is_empty() {
                                    // Stream is done and we have tool calls — transition
                                    // directly to AwaitingToolDecision.
                                    let tool_calls: Vec<ToolCall> = streaming_tool_calls
                                        .iter()
                                        .map(ToolCall::from)
                                        .collect();
                                    self.state = Some(StreamState::AwaitingToolDecision {
                                        pending_tool_calls: streaming_tool_calls,
                                        tool_responses: Vec::new(),
                                    });
                                    return Some(AgentStep::ToolRequest(tool_calls));
                                }
                                // No tool calls — continue polling; the stream will
                                // yield None next and we'll handle finish there.
                            },
                        },
                        Some(Err(e)) => {
                            let err = format!("{:#}", e);
                            error!("Stream error (attempt {}): {}", self.retry_attempt, err);

                            self.retry_attempt += 1;
                            if self.retry_attempt >= self.config.max_retries {
                                self.retry_attempt = 0;
                                self.state = None;
                                return Some(AgentStep::Error(format!(
                                    "Stream error ({}): {}", self.config.model, err
                                )));
                            }
                            // Go back to NeedsChatRequest so the retry loop picks it up
                            self.state = Some(StreamState::NeedsChatRequest);
                            return Some(AgentStep::Retrying {
                                attempt: self.retry_attempt,
                                error: err,
                            });
                        },
                        None => {
                            debug!("Agent: stream returned None (closed)");
                            // Stream ended with no (remaining) tool calls — finish.
                            match self.mode {
                                RequestMode::Compaction => {
                                    self.reset_with_summary(&self.streaming_text.clone())
                                },
                                RequestMode::Normal => {
                                    let has_content = !self.streaming_thinking.is_empty()
                                        || !self.streaming_text.is_empty();
                                    if has_content {
                                        let mut msg_content = MessageContent::default();

                                        for thinking in &self.streaming_thinking {
                                            msg_content = msg_content.append(
                                                ContentPart::Thinking(thinking.clone()),
                                            );
                                        }

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
                        },
                    }
                },

                StreamState::AwaitingToolDecision { .. } => {
                    // Blocked waiting for tool results
                    return None;
                },
            }
        }
    }

    /// Submit a tool execution result.
    ///
    /// Called by App after ToolExecutor runs the tool. The pending tool calls
    /// and accumulated responses live inside the `AwaitingToolDecision` variant,
    /// so the compiler ensures this data is only accessible in the correct state.
    pub fn submit_tool_result(&mut self, call_id: &str, content: String) {
        debug!("Agent: submit_tool_result call_id={}", call_id);

        // Extract the mutable state data from the AwaitingToolDecision variant.
        // Any other state is a caller bug — warn and bail.
        let (pending_tool_calls, tool_responses) = match &mut self.state {
            Some(StreamState::AwaitingToolDecision {
                pending_tool_calls,
                tool_responses,
            }) => (pending_tool_calls, tool_responses),
            other => {
                let state_name = match other {
                    Some(StreamState::NeedsChatRequest) => "NeedsChatRequest",
                    Some(StreamState::Streaming { .. }) => "Streaming",
                    Some(StreamState::AwaitingToolDecision { .. }) => unreachable!(),
                    None => "None",
                };
                tracing::warn!(
                    "submit_tool_result called in unexpected state: {}",
                    state_name
                );
                return;
            },
        };

        // Store the response
        tool_responses.push(ToolResponse::new(call_id.to_string(), content));

        debug!(
            "Agent: tool_responses={}/{}",
            tool_responses.len(),
            pending_tool_calls.len()
        );

        if tool_responses.len() >= pending_tool_calls.len() {
            debug!(
                "Agent: all tools complete, building message. thinking_blocks={}, text_len={}, tool_calls={}",
                self.streaming_thinking.len(),
                self.streaming_text.len(),
                pending_tool_calls.len()
            );

            // All tools processed — build the assistant message.
            // Per Anthropic docs: thinking blocks must come first, then text/tool_use.
            let mut msg_content = MessageContent::default();

            for thinking in &self.streaming_thinking {
                msg_content = msg_content.append(ContentPart::Thinking(thinking.clone()));
            }

            if !self.streaming_text.is_empty() {
                msg_content = msg_content.append(ContentPart::Text(self.streaming_text.clone()));
            }

            for tc in pending_tool_calls.iter() {
                msg_content = msg_content.append(ContentPart::ToolCall(tc.clone()));
            }

            self.messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: msg_content,
                options: None,
            });

            // Take ownership of the tool responses by swapping the state.
            // This consumes the AwaitingToolDecision data cleanly.
            let old_state = self.state.take();
            if let Some(StreamState::AwaitingToolDecision { tool_responses, .. }) = old_state {
                for response in tool_responses {
                    debug!("Agent: adding tool response - call_id={}", response.call_id);
                    self.messages.push(ChatMessage::from(response));
                }
            }

            debug!(
                "Agent: continuation will have {} messages total",
                self.messages.len()
            );

            debug!("Agent: state -> NeedsChatRequest (ready for continuation)");
            self.retry_attempt = 0;
            self.state = Some(StreamState::NeedsChatRequest);
        }
    }
}
