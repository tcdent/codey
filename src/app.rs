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
use crate::config::{Config, GeneralConfig, ToolAccess};
use crate::llm::{Agent, AgentId, AgentRegistry, AgentStep, RequestMode};
use crate::tools::{ToolDecision, ToolEffect, ToolEvent, ToolExecutor, ToolRegistry};
use crate::tool_filter::ToolFilters;
use crate::transcript::{BlockType, Role, Status, TextBlock, Transcript};
use crate::ide::{Ide, IdeEvent, Nvim};
use crate::ui::{Attachment, ChatView, InputBox};


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
const COMPACTION_PROMPT: &str = r#"The conversation context is getting large and needs to be compacted.

Please provide a comprehensive summary of our conversation so far in markdown format. Include:

1. **What was accomplished** - Main tasks and changes completed as a bulleted list
2. **What still needs to be done** - Remaining tasks or open areas of work as a bulleted list
3. **Key project information** - Important facts about the project that the user has shared or that we're not immediately apparent
4. **Relevant files** - Files most relevant to the current work with brief descriptions, line numbers, or method/variable names
5. **Relevant documentation paths or URLs** - Links to docs or resources we will use to continue our work
6. **Quotes and log snippets** - Any important quotes or logs that th euser provided that we'll need later

Be thorough but concise - this summary will seed a fresh conversation context."#;

const SUB_AGENT_PROMPT: &str = r#"You are a background research agent. Your task is to investigate, explore, or analyze as directed.

## Capabilities
You have read-only access to:
- `read_file`: Read file contents
- `shell`: Execute commands (for searching, exploring)
- `fetch_url`: Fetch web content
- `web_search`: Search the web
- `open_file`: Signal a file to open in the IDE

## Guidelines
- Focus on the specific task assigned to you
- Be thorough but concise in your findings
- Report back with structured, actionable information
- You cannot modify files - only read and explore
- If you need to suggest changes, describe them clearly for the primary agent to implement
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
    /// Chat view (owns the transcript)
    chat: ChatView,
    input: InputBox,
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
    /// Agent registry for managing multiple agents
    agents: AgentRegistry,
    /// Tool executor for managing tool approval and execution
    tool_executor: ToolExecutor,
    /// OAuth credentials for agent creation
    oauth: Option<crate::auth::OAuthCredentials>,
}

impl App {
    /// Setup terminal for TUI mode
    fn setup_terminal(stdout: &mut Stdout) -> Result<()> {
        enable_raw_mode().context("Failed to enable raw mode")?;
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

        Ok(())
    }

    /// Restore terminal to normal mode
    fn restore_terminal(&mut self) -> Result<()> {
        disable_raw_mode().context("Failed to disable raw mode")?;
        execute!(
            self.terminal.backend_mut(),
            crossterm::event::DisableBracketedPaste,
            crossterm::event::PopKeyboardEnhancementFlags
        )
        .context("Failed to restore terminal")?;
        self.terminal
            .show_cursor()
            .context("Failed to show cursor")?;

        Ok(())
    }

    /// Create a new application
    pub async fn new(config: Config, continue_session: bool) -> Result<Self> {
        // Okay so in tracing down trying to get the viewport to line up with the 
        // scroll, it looks like we need to subtract the height of the input from 
        // the viewport in order to get the height of the chat view
        // TODO All of those dimensions are in the draw method 
        let viewport_height: u16 = 12;
        let chat_height = viewport_height.saturating_sub(5) as usize;
        
        let mut stdout = io::stdout();
        Self::setup_terminal(&mut stdout)?;
        let terminal_size = crossterm::terminal::size()?;

        let backend = CrosstermBackend::new(stdout);
        // Use inline viewport for native scrollback support
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
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

        Ok(Self {
            config,
            terminal,
            chat: ChatView::new(transcript, terminal_size.0, chat_height),
            input: InputBox::new(),
            should_quit: false,
            continue_session,
            message_queue: VecDeque::new(),
            last_render: Instant::now(),
            alert: None,
            tool_filters,
            ide,
            events: EventStream::new(),
            input_mode: InputMode::Normal,
            agents: AgentRegistry::new(),
            tool_executor: ToolExecutor::new(ToolRegistry::new()),
            oauth: None,
        })
    }

    /// Run the main event loop - purely event-driven rendering
    pub async fn run(&mut self) -> Result<()> {
        self.oauth = crate::auth::OAuthCredentials::load()
            .ok()
            .flatten();

        let mut agent = Agent::new(
            self.config.general.clone(),
            SYSTEM_PROMPT,
            self.oauth.clone(),
        );

        if self.continue_session {
            agent.restore_from_transcript(&self.chat.transcript);
        } else {
            self.chat.add_turn(Role::Assistant, TextBlock::pending(WELCOME_MESSAGE));
        }
        self.agents.register(agent);

        // Initial render - populate hot zone from transcript
        self.chat.render(&mut self.terminal)?;
        self.draw()?;
        
        // CANCEL-SAFETY: When one branch of tokio::select! completes, all other
        // futures are dropped (not paused). Any async fn polled here must store
        // its state on `self`, not in local variables, so it can resume correctly
        // when a new future is created on the next loop iteration.
        loop {
            tokio::select! {
                biased;

                // Handle terminal events with highest priority (keystrokes, resize, paste)
                Some(term_event) = self.events.next() => {
                    self.handle_term_event(term_event).await?;
                }
                // Handle IDE events (selection changes)
                Some(ide_event) = async { self.ide.as_mut()?.next().await } => {
                    self.handle_ide_event(ide_event);
                }
                // Handle agent steps (streaming responses, tool requests)
                Some((agent_id, agent_step)) = self.agents.next() => {
                    self.handle_agent_step(agent_id, agent_step).await?;
                }
                // Handle tool executor events (tool output, completion)
                Some(tool_event) = self.tool_executor.next() => {
                    self.handle_tool_event(tool_event).await?;
                }
                // Handle queued messages when in normal input mode
                Some(request) = async { self.message_queue.pop_front() }, if self.input_mode == InputMode::Normal => {
                    self.handle_message(request).await?;
                }
            }

            if self.should_quit {
                break;
            }
        }

        self.restore_terminal()
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
        let context_tokens = self.agents.primary()
            .and_then(|m| m.try_lock().ok())
            .map_or(0, |a| a.total_usage().context_tokens);
        let input_widget = self.input.widget(
            &self.config.general.model,
            context_tokens,
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
            tracing::debug!("Action received: {:?}", action);
        }
        match action {
            Action::Interrupt => {
                return ActionResult::Interrupt;
            },
            Action::Quit => {
                self.should_quit = true;
                return ActionResult::Interrupt;
            }
            Action::ApproveTool => {
                self.decide_pending_tool(ToolDecision::Approve).await;
            }
            Action::DenyTool => {
                self.decide_pending_tool(ToolDecision::Deny).await;
            }
            Action::ApproveToolSession => {
                // TODO: implement allow for session
                self.decide_pending_tool(ToolDecision::Approve).await;
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
                // TODO trigger redraw with new dimensions
                return ActionResult::NoOp;
            }
        }

        ActionResult::Continue
    }

    /// Queue a user message or command for processing
    fn queue_message(&mut self, content: String) {
        let message = match Command::parse(&content) {
            Some(command) => {
                // Slash command
                let name = command.name().to_string();
                let turn_id = self.chat.add_turn(
                    Role::User, 
                    TextBlock::pending(format!("/{}", name)));
                MessageRequest::Command(name, turn_id)
            },
            None => {
                // Regular user message
                let turn_id = self.chat.add_turn(
                    Role::User, 
                    TextBlock::pending(&content));
                MessageRequest::User(content, turn_id)
            },
        };
        self.message_queue.push_back(message);

        self.chat.render(&mut self.terminal)
            .expect("Failed to render chat");
        self.draw()
            .expect("Failed to draw terminal after queuing message");
    }

    /// Queue a compaction request
    pub fn queue_compaction(&mut self) {
        // TODO push_front?
        self.message_queue.push_back(MessageRequest::Compaction);
    }

    /// Cancel the current agent request and reset input mode
    async fn cancel(&mut self) -> Result<()> {
        if let Some(agent_mutex) = self.agents.primary() {
            agent_mutex.lock().await.cancel();
        }
        self.chat.finish_turn(&mut self.terminal)?;
        self.input_mode = InputMode::Normal;
        Ok(())
    }

    /// Handle a terminal event
    async fn handle_term_event(&mut self, event: std::io::Result<Event>) -> Result<()> {
        let event = match event {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Event stream error: {}", e);
                return Ok(());
            }
        };

        let Some(action) = map_event(self.input_mode, event) else {
            return Ok(());
        };

        match self.handle_action(action).await {
            ActionResult::Interrupt => { self.cancel().await?; }
            ActionResult::Continue => { self.draw_throttled()?; }
            ActionResult::NoOp => {}
        }

        Ok(())
    }

    /// Handle an IDE event (selection changes, etc.)
    fn handle_ide_event(&mut self, event: IdeEvent) {
        match event {
            IdeEvent::SelectionChanged(selection) => {
                let attachment = selection.map(|sel| {
                    let path = std::env::current_dir()
                        .ok()
                        .and_then(|cwd| std::path::Path::new(&sel.path).strip_prefix(cwd).ok())
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or(sel.path);
                    
                    Attachment::ide_selection(path, sel.content, sel.start_line, sel.end_line)
                });
                self.input.set_ide_selection(attachment);
                let _ = self.draw_throttled();
            }
        }
    }
    
    /// Start processing a message request
    async fn handle_message(&mut self, request: MessageRequest) -> Result<()> {
        // Refresh OAuth token for primary agent if needed
        if let Some(agent_mutex) = self.agents.primary() {
            let mut agent = agent_mutex.lock().await;
            if let Err(e) = agent.refresh_oauth_if_needed().await {
                tracing::warn!("Failed to refresh OAuth token: {}", e);
            }
        }

        match request {
            MessageRequest::Command(name, turn_id) => {
                self.chat.mark_last_block_complete(turn_id);

                if let Some(command) = Command::get(&name) {
                    match command.execute(self) {
                        Ok(None) => {
                            // Command executed, no output - still need to render
                            self.chat.render(&mut self.terminal)?;
                            self.draw()?;
                        }
                        Ok(Some(output)) => {
                            let idx = self.chat.transcript.add_empty(Role::Assistant);
                            if let Some(turn) = self.chat.transcript.get_mut(idx) {
                                turn.start_block(Box::new(TextBlock::complete(&output)));
                            }
                            self.chat.render(&mut self.terminal)?;
                            self.draw()?;
                        }
                        Err(e) => {
                            tracing::error!("Command execution error: {}", e);
                            self.alert = Some(format!("Command error: {}", e));
                        }
                    }
                }
            }
            MessageRequest::User(content, turn_id) => {
                self.chat.mark_last_block_complete(turn_id);
                self.chat.render(&mut self.terminal)?;
                self.draw()?;

                if let Some(agent_mutex) = self.agents.primary() {
                    agent_mutex.lock().await.send_request(&content, RequestMode::Normal);
                }
                self.chat.begin_turn(Role::Assistant, &mut self.terminal)?;
                self.input_mode = InputMode::Streaming;
            }
            MessageRequest::Compaction => {
                if let Some(agent_mutex) = self.agents.primary() {
                    agent_mutex.lock().await.send_request(COMPACTION_PROMPT, RequestMode::Compaction);
                }
                self.chat.begin_turn(Role::Assistant, &mut self.terminal)?;
                self.input_mode = InputMode::Streaming;
            }
        }

        Ok(())
    }

    /// Handle a single agent step during streaming
    async fn handle_agent_step(&mut self, agent_id: AgentId, step: AgentStep) -> Result<()> {
        match step {
            AgentStep::TextDelta(text) => {
                self.chat.transcript.stream_delta(BlockType::Text, &text);
            }
            AgentStep::CompactionDelta(text) => {
                self.chat.transcript.stream_delta(BlockType::Compaction, &text);
            }
            AgentStep::ThinkingDelta(text) => {
                self.chat.transcript.stream_delta(BlockType::Thinking, &text);
            }
            AgentStep::ToolRequest(tool_calls) => {
                // Set agent_id on each tool call before enqueuing
                let tool_calls: Vec<_> = tool_calls
                    .into_iter()
                    .map(|tc| tc.with_agent_id(agent_id))
                    .collect();
                self.tool_executor.enqueue(tool_calls);
            }
            AgentStep::Retrying { attempt, error } => {
                self.alert = Some(format!("Request failed (attempt {}): {}. Retrying...", attempt, error));
                tracing::warn!("Retrying request: attempt {}, error: {}", attempt, error);
            }
            AgentStep::Finished { usage } => {
                self.input_mode = InputMode::Normal;

                // Handle compaction completion
                // TODO something more robust than checking active block type 
                if self.chat.transcript.is_streaming_block_type(BlockType::Compaction) {
                    self.chat.transcript.finish_turn();
                    if let Err(e) = self.chat.transcript.save() {
                        tracing::error!("Failed to save transcript before compaction: {}", e);
                    }
                    match self.chat.transcript.rotate() {
                        Ok(new_transcript) => {
                            tracing::info!("Compaction complete, rotating to {:?}", new_transcript.path());
                            self.chat.reset_transcript(new_transcript, &mut self.terminal)?;
                            self.draw()?;
                        }
                        Err(e) => {
                            tracing::error!("Failed to rotate transcript: {}", e);
                        }
                    }
                } else {
                    // Normal completion
                    self.chat.transcript.finish_turn();
                    if let Err(e) = self.chat.transcript.save() {
                        tracing::error!("Failed to save transcript: {}", e);
                    }

                    // Check if compaction is needed
                    if usage.context_tokens >= self.config.general.compaction_threshold {
                        self.queue_compaction();
                    }
                }
            }
            AgentStep::Error(msg) => {
                self.chat.transcript.mark_active_block(Status::Error);
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
            }
        }

        // Update display
        self.chat.render(&mut self.terminal)?;
        self.draw_throttled()?;

        Ok(())
    }

    /// Execute a tool decision (approve/deny) for the current tool
    async fn decide_pending_tool(&mut self, decision: ToolDecision) {
        // Get current tool call_id
        // TODO I'd prefer to reference the call_id explictly
        let call_id = match self.tool_executor.front() {
            Some(tc) => tc.call_id.clone(),
            None => {
                tracing::warn!("No current tool to execute decision for");
                return;
            }
        };

        // Close IDE preview
        // TODO: Would be nice to know if the tool actually had an IDE preview open
        if let Some(ide) = &self.ide {
            if let Err(e) = ide.close_preview().await {
                tracing::warn!("Failed to close IDE preview: {}", e);
            }
        }

        // Update block status to Running/Denied
        self.chat.transcript.mark_active_block(match decision {
            ToolDecision::Approve => Status::Running,
            ToolDecision::Deny => Status::Denied,
            _ => unreachable!(),
        });
        let _ = self.chat.render(&mut self.terminal);
        let _ = self.draw();

        // Mark decision on the tool - next poll will execute it
        self.tool_executor.decide(&call_id, decision);
    }

    /// Handle events from the tool executor
    async fn handle_tool_event(&mut self, event: ToolEvent) -> Result<()> {
        match event {
            ToolEvent::AwaitingApproval(tool_call) => {
                // Display tool in transcript
                self.draw()?; // there's sometimes a missing token otherwise
                let tool = self.tool_executor.tools().get(&tool_call.name);
                self.chat.start_block(
                    tool.create_block(&tool_call.call_id, tool_call.params.clone()),
                    &mut self.terminal)?;

                // Show preview in IDE if the tool provides one
                if let Some(preview) = tool.ide_preview(&tool_call.params) {
                    if let Some(ide) = &self.ide {
                        if let Err(e) = ide.show_preview(&preview).await {
                            tracing::warn!("Failed to show IDE preview: {}", e);
                        }
                    }
                }
                self.draw()?;
                
                match self.tool_filters.evaluate(&tool_call.name, &tool_call.params) {
                    Some(decision) => {
                        // Auto-approve/deny based on filters
                        self.decide_pending_tool(decision).await;
                    }
                    None => {
                        // Ask user for approval
                        self.input_mode = InputMode::ToolApproval;
                    }
                }

            }

            ToolEvent::OutputDelta { call_id, delta, .. } => {
                // Stream output to the active tool block
                if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                    block.append_text(&delta);
                    // Re-render to show the delta
                    self.chat.render(&mut self.terminal)?;
                    self.draw()?;
                } else {
                    tracing::warn!("No block found for call_id: {}", call_id);
                }
            }

            ToolEvent::Completed { agent_id, call_id, content, is_error, ide_post_actions, effects } => {
                // Update transcript status (content was already streamed via OutputDelta)
                self.chat.transcript.mark_active_block(
                    if is_error { Status::Error } else { Status::Complete }
                );

                // Execute post-actions from the tool
                for action in ide_post_actions {
                    if let Some(ide) = &self.ide {
                        if let Err(e) = ide.execute(&action).await {
                            tracing::warn!("Failed to execute IDE action: {}", e);
                        }
                    }
                }

                // Process tool effects
                for effect in effects {
                    self.apply_effect(agent_id, effect).await?;
                }

                // Tell agent about the result - route to the correct agent by ID
                if let Some(agent_mutex) = self.agents.get(agent_id) {
                    agent_mutex.lock().await.submit_tool_result(&call_id, content);
                }

                // Tool is done - go back to streaming mode.
                // If there's another tool pending, AwaitingApproval will switch back.
                self.input_mode = InputMode::Streaming;

                // Render update
                self.chat.render(&mut self.terminal)?;
                self.draw()?;
            }
        }
        Ok(())
    }

    /// Apply a tool effect
    /// Effects are side-effects requested by tools, processed after tool completion
    async fn apply_effect(&mut self, _agent_id: AgentId, effect: ToolEffect) -> Result<()> {
        match effect {
            ToolEffect::SpawnAgent { task, context } => {
                tracing::info!("SpawnAgent effect: task={}, context={:?}", task, context);

                // Build sub-agent config from main config
                let sub_config = &self.config.general.sub_agent;
                let agent_config = GeneralConfig {
                    model: sub_config.model.clone()
                        .unwrap_or_else(|| self.config.general.model.clone()),
                    max_tokens: sub_config.max_tokens,
                    thinking_budget: sub_config.thinking_budget,
                    // Inherit other settings from primary
                    working_dir: self.config.general.working_dir.clone(),
                    max_retries: self.config.general.max_retries,
                    compaction_threshold: self.config.general.compaction_threshold,
                    compaction_thinking_budget: self.config.general.compaction_thinking_budget,
                    sub_agent: self.config.general.sub_agent.clone(),
                };

                // Choose tool registry based on access level
                let tools = match sub_config.tool_access {
                    ToolAccess::Full => ToolRegistry::new(),
                    ToolAccess::ReadOnly => ToolRegistry::read_only(),
                    ToolAccess::None => ToolRegistry::empty(),
                };

                // Build system prompt with context
                let system_prompt = if let Some(ctx) = &context {
                    format!("{}\n\n## Context\n{}", SUB_AGENT_PROMPT, ctx)
                } else {
                    SUB_AGENT_PROMPT.to_string()
                };

                // Create and register the sub-agent
                let mut sub_agent = Agent::with_tools(
                    agent_config,
                    &system_prompt,
                    self.oauth.clone(),
                    tools,
                );

                // Send the task to the sub-agent
                sub_agent.send_request(&task, RequestMode::Normal);

                let sub_agent_id = self.agents.register(sub_agent);
                tracing::info!("Spawned sub-agent {} for task: {}", sub_agent_id, task);
            }
            ToolEffect::IdeOpen { path, line } => {
                if let Some(ide) = &self.ide {
                    use crate::ide::IdeAction;
                    let action = IdeAction::NavigateTo {
                        path: path.to_string_lossy().to_string(),
                        line,
                        column: None,
                    };
                    if let Err(e) = ide.execute(&action).await {
                        tracing::warn!("Failed to open file in IDE: {}", e);
                    }
                }
            }
            ToolEffect::Notify { message } => {
                self.alert = Some(message);
            }
        }
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.restore_terminal();
    }
}


