use std::io::{self, Stdout};
use std::time::{Duration, Instant};
use std::collections::VecDeque;

use anyhow::{Context, Result};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal, TerminalOptions, Viewport,
};

use crate::commands::Command;
use crate::config::Config;
use crate::llm::{Agent, AgentStep, RequestMode, Usage};
use crate::tools::{ToolDecision, ToolEvent, ToolExecutor, ToolRegistry};
use crate::tool_filter::ToolFilters;
use crate::transcript::{BlockType, Role, Status, TextBlock, Transcript};
use crate::compaction::COMPACTION_PROMPT;
use crate::ide::{Ide, Nvim};
use crate::ui::{Attachment, ChatView, ConnectionStatus, InputBox};


const MIN_FRAME_TIME: Duration = Duration::from_millis(16);

pub const APP_NAME: &str = "Codey";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CODEY_DIR: &str = ".codey";
pub const TRANSCRIPTS_DIR: &str = "transcripts";

const WELCOME_MESSAGE: &str = "Welcome to Codey! I'm your AI coding assistant. How can I help you today?";
const SYSTEM_PROMPT: &str = r#"You are Codey, an AI coding assistant running in a terminal interface.

## Capabilities
You have access to the following tools:
- `read_file`: Read file contents, optionally with line ranges
- `write_file`: Create new files
- `edit_file`: Make precise edits using search/replace
- `shell`: Execute bash commands
- `fetch_url`: Fetch web content

## Guidelines

### Reading Files
- Always read a file before editing it
- Use line ranges for large files: `read_file(path, start_line=100, end_line=200)`
- Use `shell("ls -la")` to explore directories
- When reading files, be careful about reading large files in one-go. Use line ranges, 
    or check the file stats with `shell("stat <file_path>")` first.
- shell grep is a great way to get a line number to read a targeted section of a file

### Editing Files
- Use `edit_file` for existing files, `write_file` only for new files
- The `old_string` must match EXACTLY, including whitespace and indentation
- If `old_string` appears multiple times, include more context to make it unique
- Apply edits sequentially; each edit sees the result of previous edits
- UYou can do multiple edits at once, but keep it under 1000 lines

### Shell Commands
- Prefer `read_file` over `cat`, `head`, `tail`
- Use `ls` for directory exploration
- Use `grep` or `rg` for searching code
- `pwd` is an easy way to remind yourself of your current directory
- Only prepend a command with cd <some/dir> && if you really need to change directories

### General
- Be concise but thorough
- Explain what you're doing before executing tools
- If a tool fails, explain the error and suggest fixes
- Ask for clarification if the request is ambiguous
- If you feel like backing out of a path, always get confirmation before git resetting the work tree
- Always get confirmation before making destructive changes (this includes building a release)
"#;

/// Result of handling an action
enum ActionResult {
    NoOp,
    Continue,
    Interrupt,
}

/// Input modes determine which keybindings are active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Streaming,
    ToolApproval,
}

/// Types of messages that can be processed through the event loop
#[derive(Debug, Clone)]
enum MessageRequest {
    /// Regular user message (content, turn_id)
    User(String, usize),
    /// Compaction request (triggered when context exceeds threshold)
    Compaction,
    /// Command execution (command_name, turn_id)
    Command(String, usize),
}

/// Actions that can be triggered by terminal events
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    // Text input
    InsertChar(char),
    InsertNewline,
    DeleteBack,
    Paste(String),
    // Cursor movement
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
    // Input control
    Submit,
    ClearInput,
    HistoryPrev,
    HistoryNext,
    TabComplete,
    // Application control
    Interrupt,
    Quit,
    Resize(u16, u16),
    // Tool approval
    ApproveTool,
    DenyTool,
    ApproveToolSession,
}

/// Map a terminal event to an action based on the current input mode
fn map_event(mode: InputMode, event: Event) -> Option<Action> {
    match event {
        Event::Key(key) => map_key(mode, key),
        Event::Paste(content) => Some(Action::Paste(content)),
        Event::Resize(w, h) => Some(Action::Resize(w, h)),
        _ => None,
    }
}

/// Map a key event to an action based on the current input mode
fn map_key(mode: InputMode, key: KeyEvent) -> Option<Action> {
    match mode {
        InputMode::Normal => map_key_normal(key),
        InputMode::Streaming => map_key_streaming(key),
        InputMode::ToolApproval => map_key_tool_approval(key),
    }
}

/// Keybindings for normal input mode
fn map_key_normal(key: KeyEvent) -> Option<Action> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char(c) => Some(Action::InsertChar(c)),
        KeyCode::Backspace => Some(Action::DeleteBack),
        KeyCode::Left => Some(Action::CursorLeft),
        KeyCode::Right => Some(Action::CursorRight),
        KeyCode::Home => Some(Action::CursorHome),
        KeyCode::End => Some(Action::CursorEnd),
        KeyCode::Enter if shift || alt => Some(Action::InsertNewline),
        KeyCode::Enter => Some(Action::Submit),
        KeyCode::Esc => Some(Action::ClearInput),
        KeyCode::Up => Some(Action::HistoryPrev),
        KeyCode::Down => Some(Action::HistoryNext),
        KeyCode::Tab => Some(Action::TabComplete),
        _ => None,
    }
}

/// Keybindings for streaming input mode
fn map_key_streaming(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Interrupt),
        _ => map_key_normal(key),
    }
}

/// Keybindings for tool approval mode
fn map_key_tool_approval(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => Some(Action::ApproveTool),
        KeyCode::Char('n') | KeyCode::Esc => Some(Action::DenyTool),
        KeyCode::Char('a') => Some(Action::ApproveToolSession),
        _ => None,
    }
}

/// Application state
pub struct App {
    config: Config,
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Conversation history
    transcript: Transcript,
    /// UI views
    chat: ChatView,
    input: InputBox,
    /// Connection status to the LLM service (TODO unused)
    status: ConnectionStatus,
    /// LLM usage statistics (TODO: this should live on Agent)
    usage: Usage,
    /// Flag to indicate a quit request
    should_quit: bool,
    continue_session: bool,
    /// Queue of messages waiting to be processed
    message_queue: VecDeque<MessageRequest>,
    /// Last render time for frame rate limiting
    last_render: Instant,
    /// Alert message to display (cleared on next user input)
    alert: Option<String>,
    /// Compiled tool parameter filters for auto-approve/deny
    tool_filters: ToolFilters,
    /// IDE connection for editor integration (e.g., Neovim)
    ide: Option<Box<dyn Ide>>,
    /// Terminal event stream
    events: EventStream,
    /// Current input mode
    input_mode: InputMode,
    /// LLM agent for conversation
    agent: Option<Agent>,
    /// Tool executor for managing tool approval and execution
    tool_executor: ToolExecutor,
}

impl App {
    /// Create a new application
    pub async fn new(config: Config, continue_session: bool) -> Result<Self> {
        // Setup terminal
        enable_raw_mode().context("Failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            crossterm::terminal::SetTitle(format!("{} v{}", APP_NAME, APP_VERSION)),
            crossterm::event::EnableBracketedPaste,
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )
        .context("Failed to setup terminal")?;

        let backend = CrosstermBackend::new(stdout);
        let terminal_size = crossterm::terminal::size()?;
        let VIEWPORT_HEIGHT = 12; // TODO: make configurable
        
        // Use inline viewport for native scrollback support
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(VIEWPORT_HEIGHT),
            }
        ).context("Failed to create terminal")?;

        // Load existing transcript or create new one
        let transcript = if continue_session {
            Transcript::load()
                .context("Failed to load transcript")?
        } else {
            Transcript::new_numbered()
                .context("Failed to create new transcript")?
        };

        // Compile tool filters from config
        let tool_filters = ToolFilters::compile(&config.tools.filters())
            .context("Failed to compile tool filters")?;

        // Try to connect to neovim if enabled
        let ide: Option<Box<dyn Ide>> = if config.ide.nvim.enabled {
            match Nvim::discover(&config.ide.nvim).await {
                Ok(Some(nvim)) => {
                    tracing::info!("Connected to {} at {:?}", nvim.name(), nvim.socket_path());
                    Some(Box::new(nvim))
                }
                Ok(None) => {
                    tracing::debug!("No nvim instance found");
                    None
                }
                Err(e) => {
                    tracing::warn!("Failed to connect to nvim: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Okay so in tracing down trying to get the viewport to line up with the 
        // scroll, it looks like we need to subtract the height of the input from 
        // the viewport in order to get the height of the chat view
        // TODO All of those dimensions are in the draw method 
        let chat_height = VIEWPORT_HEIGHT.saturating_sub(5) as usize;

        Ok(Self {
            config,
            terminal,
            transcript,
            chat: ChatView::new(chat_height),
            input: InputBox::new(),
            status: ConnectionStatus::Disconnected,
            usage: Usage::default(),
            should_quit: false,
            continue_session,
            message_queue: VecDeque::new(),
            last_render: Instant::now(),
            alert: None,
            tool_filters,
            ide,
            events: EventStream::new(),
            input_mode: InputMode::Normal,
            agent: None,
            tool_executor: ToolExecutor::new(ToolRegistry::new()),
        })
    }

    /// Run the main event loop - purely event-driven rendering
    pub async fn run(&mut self) -> Result<()> {
        let oauth = crate::auth::OAuthCredentials::load()
            .ok()
            .flatten();
        
        let mut agent = Agent::new(
            &self.config.general.model,
            SYSTEM_PROMPT,
            self.config.general.max_tokens,
            self.config.general.max_retries,
            oauth,
        );

        if self.continue_session {
            agent.restore_from_transcript(&self.transcript);
        } else {
            self.transcript.add_turn(Role::Assistant, TextBlock::pending(WELCOME_MESSAGE));
        }
        self.agent = Some(agent);
        self.status = ConnectionStatus::Connected;

        self.draw()?;

        // Initial render - populate hot zone from transcript
        let size = self.terminal.size()?;
        self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal)?;
        self.draw()?;
        
        loop {
            // Start any queued messages (non-blocking)
            if let Some(request) = self.message_queue.pop_front() {
                self.start_message(request).await?;
            }

            tokio::select! {
                biased;  // Check events first for responsive interrupts
                
                Some(term_event) = self.events.next() => {
                    match term_event {
                        Ok(event) => {
                            if let Some(action) = map_event(self.input_mode, event) {
                                match self.handle_action(action).await {
                                    ActionResult::Interrupt => {
                                        if let Some(agent) = self.agent.as_mut() {
                                            agent.cancel();
                                        }
                                        self.transcript.finish_turn();
                                        self.input_mode = InputMode::Normal;
                                        //self.tool_executor.clear();
                                    }
                                    ActionResult::Continue => { self.draw_throttled()?; }
                                    ActionResult::NoOp => {}
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Event stream error: {}", e);
                        }
                    }
                }
                
                Some(agent_step) = async { self.agent.as_mut()?.next().await } => {
                    self.handle_agent_step(agent_step).await?;
                }

                Some(tool_event) = self.tool_executor.next() => {
                    self.handle_tool_event(tool_event).await?;
                }
            }

            if self.should_quit {
                break;
            }
        }

        self.cleanup()
    }

    /// Draw the UI
    fn draw(&mut self) -> Result<()> {
        use ratatui::style::{Color, Style};
        use ratatui::widgets::Paragraph;
        
        self.last_render = Instant::now();

        // Calculate dimensions
        let size = self.terminal.size()?;
        let input_height = self.input.required_height(size.width);
        let max_input_height = size.height / 2;
        let input_height = input_height.min(max_input_height).max(5);

        // Draw the viewport (hot zone content + input)
        let chat_widget = self.chat.widget();
        let input_widget = self.input.widget(
            &self.config.general.model,
            self.usage.context_tokens,
        );
        let alert = self.alert.clone();

        self.terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(5),               // Chat area (minimum)
                    Constraint::Length(input_height), // Input area (dynamic)
                    Constraint::Length(if alert.is_some() { 1 } else { 0 }),
                ])
                .split(frame.area());

            frame.render_widget(chat_widget, chunks[0]);
            frame.render_widget(input_widget, chunks[1]);
           
            // TODO build as an actual widget on self.alert
            if let Some(ref msg) = alert {
                let alert_widget = Paragraph::new(msg.as_str())
                    .style(Style::default().fg(Color::Red));
                frame.render_widget(alert_widget, chunks[2]);
            }
        })?;

        Ok(())
    }

    /// Draw with frame rate limiting - skips if called too frequently
    /// Returns true if a draw actually occurred
    fn draw_throttled(&mut self) -> Result<bool> {
        if self.last_render.elapsed() < MIN_FRAME_TIME {
            return Ok(false);
        }

        self.draw()?;
        Ok(true)
    }

    /// Handle an action. Returns the result indicating what the main loop should do.
    async fn handle_action(&mut self, action: Action) -> ActionResult {
        // Clear alert on any input action
        self.alert = None;
        
        if !matches!(action, Action::InsertChar(_)) {
            tracing::debug!("Current input content: {}", self.input.content());
        }
        match action {
            Action::Interrupt => return ActionResult::Interrupt,
            Action::Quit => {
                self.should_quit = true;
                return ActionResult::Interrupt;
            }
            Action::ApproveTool => {
                self.execute_tool_decision(ToolDecision::Approve).await;
            }
            Action::DenyTool => {
                self.execute_tool_decision(ToolDecision::Deny).await;
            }
            Action::ApproveToolSession => {
                // TODO: implement allow for session
                self.execute_tool_decision(ToolDecision::Approve).await;
            }
            Action::InsertChar(c) => self.input.insert_char(c),
            Action::InsertNewline => self.input.insert_newline(),
            Action::DeleteBack => self.input.delete_char(),
            Action::Paste(content) => {
                self.input.add_attachment(Attachment::pasted(content));
            }
            Action::CursorLeft => self.input.move_cursor_left(),
            Action::CursorRight => self.input.move_cursor_right(),
            Action::CursorHome => self.input.move_cursor_start(),
            Action::CursorEnd => self.input.move_cursor_end(),
            Action::Submit => {
                let content = self.input.submit();
                if !content.trim().is_empty() {
                    self.queue_message(content);
                }
            }
            Action::ClearInput => self.input.clear(),
            Action::HistoryPrev => {
                self.input.history_prev();
            }
            Action::HistoryNext => {
                self.input.history_next();
            }
            Action::TabComplete => {
                if let Some(completed) = Command::complete(&self.input.content()) {
                    self.input.set_content(&completed);
                }
            }
            Action::Resize(_w, _h) => {
                // Terminal resized - just trigger a redraw
            }
        }

        ActionResult::Continue
    }

    fn queue_message(&mut self, content: String) {
        let message = match Command::parse(&content) {
            Some(command) => {
                // Slash command
                let name = command.name().to_string();
                let turn_id = self.transcript.add_turn(
                    Role::User, 
                    TextBlock::pending(&format!("/{}", name)));
                MessageRequest::Command(name, turn_id)
            },
            None => {
                // Regular user message
                let turn_id = self.transcript.add_turn(
                    Role::User, 
                    TextBlock::pending(&content));
                MessageRequest::User(content, turn_id)
            },
        };
        self.message_queue.push_back(message);

        // TODO fix this when we clean up render calls. 
        let size = self.terminal.size()
            .expect("Failed to get terminal size");
        self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal)
            .expect("Failed to render chat to scrollback");
        self.draw()
            .expect("Failed to draw terminal after queuing message");
    }

    pub fn queue_compaction(&mut self) {
        // TODO push_front?
        self.message_queue.push_back(MessageRequest::Compaction);
    }
    
    /// Start processing a message (non-blocking)
    /// Sets up the agent to stream and changes input mode
    async fn start_message(&mut self, request: MessageRequest) -> Result<()> {
        if let Some(agent) = self.agent.as_mut() {
            if let Err(e) = agent.refresh_oauth_if_needed().await {
                tracing::warn!("Failed to refresh OAuth token: {}", e);
            }
        }

        match request {
            MessageRequest::Command(name, turn_id) => {
                // Commands are handled synchronously (no streaming)
                self.transcript.get_mut(turn_id)
                    .and_then(|turn| turn.content.first_mut())
                    .map(|block| block.set_status(Status::Complete));

                if let Some(command) = Command::get(&name) {
                    match command.execute(self) {
                        Ok(None) => {}
                        Ok(Some(output)) => {
                            let idx = self.transcript.add_empty(Role::Assistant);
                            if let Some(turn) = self.transcript.get_mut(idx) {
                                turn.start_block(Box::new(TextBlock::complete(&output)));
                            }
                            self.draw()?;
                        }
                        Err(e) => {
                            self.alert = Some(format!("Command error: {}", e));
                        }
                    }
                }
            }
            MessageRequest::User(content, turn_id) => {
                // Mark user turn complete
                self.transcript.get_mut(turn_id)
                    .and_then(|turn| turn.content.first_mut())
                    .map(|block| block.set_status(Status::Complete));

                // Render user message
                let size = self.terminal.size()?;
                self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal)?;
                self.draw()?;

                // Start streaming (non-blocking)
                if let Some(agent) = self.agent.as_mut() {
                    agent.start_response(&content, RequestMode::Normal);
                }
                self.transcript.begin_turn(Role::Assistant);
                self.input_mode = InputMode::Streaming;
            }
            MessageRequest::Compaction => {
                if let Some(agent) = self.agent.as_mut() {
                    agent.start_response(COMPACTION_PROMPT, RequestMode::Compaction);
                }
                self.transcript.begin_turn(Role::Assistant);
                self.input_mode = InputMode::Streaming;
            }
        }

        Ok(())
    }

    /// Handle a single agent step during streaming
    async fn handle_agent_step(&mut self, step: AgentStep) -> Result<()> {
        match step {
            AgentStep::TextDelta(text) => {
                self.transcript.stream_delta(BlockType::Text, &text);
            }
            AgentStep::CompactionDelta(text) => {
                self.transcript.stream_delta(BlockType::Compaction, &text);
            }
            AgentStep::ThinkingDelta(text) => {
                self.transcript.stream_delta(BlockType::Thinking, &text);
            }
            AgentStep::ToolRequest(tool_calls) => {
                // Just enqueue - the select loop will poll tool_executor.next()
                tracing::debug!("Agent requested tool calls: {:?}", tool_calls);
                self.tool_executor.enqueue(tool_calls);
            }
            AgentStep::Retrying { attempt, error } => {
                self.status = ConnectionStatus::Error(format!("Retry {} - {}", attempt, error));
            }
            AgentStep::Finished { usage, thinking_signatures: _ } => {
                tracing::debug!("AgentStep::Finished received");
                self.usage = usage;
                self.status = ConnectionStatus::Connected;
                self.input_mode = InputMode::Normal;

                // Handle compaction completion
                if self.transcript.is_streaming_block_type(BlockType::Compaction) {
                    tracing::debug!("Finishing compaction turn");
                    self.transcript.finish_turn();
                    if let Err(e) = self.transcript.save() {
                        tracing::error!("Failed to save transcript before compaction: {}", e);
                    }
                    match self.transcript.rotate() {
                        Ok(new_transcript) => {
                            self.transcript = new_transcript;
                            tracing::info!("Compaction complete, rotated to {:?}", self.transcript.path());
                        }
                        Err(e) => {
                            tracing::error!("Failed to rotate transcript: {}", e);
                        }
                    }
                } else {
                    // Normal completion
                    tracing::debug!("Finishing normal turn");
                    self.transcript.finish_turn();
                    if let Err(e) = self.transcript.save() {
                        tracing::error!("Failed to save transcript: {}", e);
                    }

                    // Check if compaction is needed
                    if self.usage.context_tokens >= self.config.general.compaction_threshold {
                        self.queue_compaction();
                    }
                }
            }
            AgentStep::Error(msg) => {
                self.transcript.mark_active_block(Status::Error);
                self.input_mode = InputMode::Normal;

                let alert_msg = if let Some(start) = msg.find('{') {
                    serde_json::from_str::<serde_json::Value>(&msg[start..])
                        .ok()
                        .and_then(|json| json["error"]["message"].as_str().map(String::from))
                        .unwrap_or_else(|| msg.clone())
                } else {
                    msg.clone()
                };
                self.alert = Some(alert_msg);
                self.status = ConnectionStatus::Error(msg);
            }
        }

        // Update display
        let size = self.terminal.size()?;
        self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal)?;
        self.draw_throttled()?;

        Ok(())
    }

    /// Execute a tool decision (approve/deny) for the current tool
    ///
    /// This starts execution; the result will come via handle_tool_event.
    async fn execute_tool_decision(&mut self, decision: ToolDecision) {
        // Get current tool call_id
        tracing::debug!("Executing tool decision: {:?}", decision);
        let call_id = match self.tool_executor.front() {
            Some(tc) => tc.call_id.clone(),
            None => {
                tracing::warn!("No current tool to execute decision for");
                return;
            }
        };

        // Close IDE preview
        if let Some(ide) = &self.ide {
            if let Err(e) = ide.close_preview().await {
                tracing::warn!("Failed to close IDE preview: {}", e);
            }
        }

        // Update block status to Running/Denied
        self.transcript.mark_active_block(match decision {
            ToolDecision::Approve => Status::Running,
            ToolDecision::Deny => Status::Denied,
            _ => unreachable!(),
        });
        if let Ok(size) = self.terminal.size() {
            let _ = self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal);
        }
        let _ = self.draw();

        // Mark decision on the tool - next poll will execute it
        self.tool_executor.decide(&call_id, decision);
    }

    /// Handle events from the tool executor
    async fn handle_tool_event(&mut self, event: ToolEvent) -> Result<()> {
        tracing::debug!("Handling ToolEvent: {:?}", event);
        match event {
            ToolEvent::AwaitingApproval(tool_call) => {
                // Display tool in transcript
                let tool = self.tool_executor.tools().get(&tool_call.name);
                self.transcript.start_block(
                    tool.create_block(&tool_call.call_id, tool_call.params.clone()));

                // Show preview in IDE if the tool provides one
                if let Some(preview) = tool.preview(&tool_call.params) {
                    if let Some(ide) = &self.ide {
                        if let Err(e) = ide.show_preview(&preview).await {
                            tracing::warn!("Failed to show IDE preview: {}", e);
                        }
                    }
                }

                let size = self.terminal.size()?;
                self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal)?;
                self.draw()?;

                // Check if tool filter auto-approves/denies
                if let Some(decision) = self.tool_filters.evaluate(&tool_call.name, &tool_call.params) {
                    self.execute_tool_decision(decision).await;
                } else {
                    self.input_mode = InputMode::ToolApproval;
                }
            }

            ToolEvent::OutputDelta { call_id, delta } => {
                // Stream output to the active tool block
                if let Some(block) = self.transcript.find_tool_block_mut(&call_id) {
                    tracing::debug!("Found block, appending text");
                    block.append_text(&delta);
                    // Re-render to show the delta
                    let size = self.terminal.size()?;
                    self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal)?;
                    self.draw()?;
                } else {
                    tracing::warn!("No block found for call_id: {}", call_id);
                }
            }

            ToolEvent::Completed { call_id, content, is_error, post_actions } => {
                // Update transcript status (content was already streamed via OutputDelta)
                self.transcript.mark_active_block(
                    if is_error { Status::Error } else { Status::Complete }
                );

                // Execute post-actions from the tool
                for action in post_actions {
                    if let Some(ide) = &self.ide {
                        if let Err(e) = ide.execute(&action).await {
                            tracing::warn!("Failed to execute IDE action: {}", e);
                        }
                    }
                }

                // Tell agent about the result (for message history)
                if let Some(agent) = self.agent.as_mut() {
                    agent.submit_tool_result(&call_id, content);
                }

                // Render update
                let size = self.terminal.size()?;
                self.chat.render_to_scrollback(&self.transcript, size.width, &mut self.terminal)?;
                self.draw()?;
            }
        }
        Ok(())
    }

    /// Cleanup terminal
    fn cleanup(&mut self) -> Result<()> {
        disable_raw_mode().context("Failed to disable raw mode")?;
        execute!(
            self.terminal.backend_mut(),
            crossterm::event::DisableBracketedPaste,
            crossterm::event::PopKeyboardEnhancementFlags
        )
        .context("Failed to cleanup terminal")?;
        self.terminal
            .show_cursor()
            .context("Failed to show cursor")?;

        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}


