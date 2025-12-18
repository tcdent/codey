use crate::compaction::{CompactionBlock, COMPACTION_PROMPT};
use crate::config::Config;
use crate::ide::{Ide, Nvim};
use crate::llm::{Agent, AgentStep, RequestMode, ToolDecision, Usage};
use crate::tool_filter::ToolFilters;
use crate::transcript::{BlockType, Role, Status, TextBlock, ThinkingBlock, Transcript};
use crate::tools::ToolRegistry;
use crate::ui::{ChatView, ConnectionStatus, InputBox};

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

use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind},
    execute, queue,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};
use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant};

const APP_NAME: &str = "Codey";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CODEY_DIR: &str = ".codey";
pub const TRANSCRIPTS_DIR: &str = "transcripts";
const MIN_FRAME_TIME: Duration = Duration::from_millis(16);



/// Tracks the currently active block during streaming
/// Input modes determine which keybindings are active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Streaming,
    ToolApproval,
}

/// Result of polling for events during streaming
enum PollResult {
    NoEvent,
    Handled,
    Interrupted,
}

/// Actions that can be triggered by key events
#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    InsertChar(char),
    InsertNewline,
    DeleteBack,
    DeleteForward,
    CursorLeft,
    CursorRight,
    CursorHome,
    CursorEnd,
    Submit,
    ClearInput,
    HistoryPrev,
    HistoryNext,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    Interrupt,
    Quit,
    ApproveTool,
    DenyTool,
    ApproveToolSession,
    TabComplete,
}

/// Map a key event to an action based on the current input mode
fn map_key(mode: InputMode, key: KeyEvent) -> Option<Action> {
    match mode {
        InputMode::Normal => map_key_normal(key),
        InputMode::Streaming => map_key_streaming(key),
        InputMode::ToolApproval => map_key_tool_approval(key),
    }
}

fn map_key_normal(key: KeyEvent) -> Option<Action> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Some(Action::Quit),
            KeyCode::Up => Some(Action::ScrollUp),
            KeyCode::Down => Some(Action::ScrollDown),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char(c) => Some(Action::InsertChar(c)),
        KeyCode::Backspace => Some(Action::DeleteBack),
        KeyCode::Delete => Some(Action::DeleteForward),
        KeyCode::Left => Some(Action::CursorLeft),
        KeyCode::Right => Some(Action::CursorRight),
        KeyCode::Home => Some(Action::CursorHome),
        KeyCode::End => Some(Action::CursorEnd),
        KeyCode::Enter if shift || alt => Some(Action::InsertNewline),
        KeyCode::Enter => Some(Action::Submit),
        KeyCode::Esc => Some(Action::ClearInput),
        KeyCode::Up => Some(Action::HistoryPrev),
        KeyCode::Down => Some(Action::HistoryNext),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        KeyCode::Tab => Some(Action::TabComplete),
        _ => None,
    }
}

fn map_key_streaming(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Interrupt),
        _ => map_key_normal(key),
    }
}

fn map_key_tool_approval(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => Some(Action::ApproveTool),
        KeyCode::Char('n') | KeyCode::Esc => Some(Action::DenyTool),
        KeyCode::Char('a') => Some(Action::ApproveToolSession),
        _ => None,
    }
}

fn map_mouse(mouse: MouseEvent) -> Option<Action> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(Action::ScrollUp),
        MouseEventKind::ScrollDown => Some(Action::ScrollDown),
        _ => None,
    }
}

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

### Editing Files
- Use `edit_file` for existing files, `write_file` only for new files
- The `old_string` must match EXACTLY, including whitespace and indentation
- If `old_string` appears multiple times, include more context to make it unique
- Apply edits sequentially; each edit sees the result of previous edits

### Shell Commands
- Prefer `read_file` over `cat`, `head`, `tail`
- Use `ls` for directory exploration
- Use `grep` or `rg` for searching code
- `pwd` is an easy way to remind yourself of your current directory

### General
- Be concise but thorough
- Explain what you're doing before executing tools
- If a tool fails, explain the error and suggest fixes
- Ask for clarification if the request is ambiguous
"#;

/// Application state
pub struct App {
    config: Config,
    terminal: Terminal<CrosstermBackend<Stdout>>,
    transcript: Transcript,
    chat: ChatView,
    input: InputBox,
    status: ConnectionStatus,
    usage: Usage,
    should_quit: bool,
    continue_session: bool,
    /// Queue of messages waiting to be processed
    message_queue: Vec<MessageRequest>,
    /// Last render time for frame rate limiting
    last_render: Instant,
    /// Alert message to display (cleared on next user input)
    alert: Option<String>,
    /// Compiled tool parameter filters for auto-approve/deny
    tool_filters: ToolFilters,
    /// IDE connection for editor integration (e.g., Neovim)
    ide: Option<Box<dyn Ide>>,
}

impl App {
    /// Create a new application
    pub async fn new(config: Config, continue_session: bool) -> Result<Self> {
        // Setup terminal
        enable_raw_mode().context("Failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            crossterm::terminal::SetTitle(format!("{} v{}", APP_NAME, APP_VERSION)),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )
        .context("Failed to setup terminal")?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("Failed to create terminal")?;

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
            transcript,
            chat: ChatView::new(),
            input: InputBox::new(),
            status: ConnectionStatus::Disconnected,
            usage: Usage::default(),
            should_quit: false,
            continue_session,
            message_queue: Vec::new(),
            last_render: Instant::now(),
            alert: None,
            tool_filters,
            ide,
        })
    }

    /// Run the main event loop - purely event-driven rendering
    pub async fn run(&mut self) -> Result<()> {
        let tools = ToolRegistry::new();
        // Load OAuth credentials if available
        let oauth = crate::auth::OAuthCredentials::load()
            .ok()
            .flatten();
        if oauth.is_some() {
            tracing::info!("Using OAuth authentication");
        }
        
        let mut agent = Agent::new(
            &self.config.general.model,
            self.config.general.max_tokens,
            self.config.general.max_retries,
            SYSTEM_PROMPT,
            tools,
            oauth,
        );

        // Restore agent context if continuing session
        if self.continue_session && !self.transcript.turns().is_empty() {
            agent.restore_from_transcript(&self.transcript);
        } else {
            // Show welcome message only for new sessions
            self.transcript.add_turn(
                Role::Assistant,
                TextBlock::new(
                    "Welcome to Codey! I'm your AI coding assistant. How can I help you today?",
                ),
            );
        }
        self.status = ConnectionStatus::Connected;

        // Initial render
        self.draw()?;

        // Main event loop - only renders on actual events
        loop {
            // Process queued messages first (agent events trigger their own draws)
            if let Some(request) = self.message_queue.first().cloned() {
                self.message_queue.remove(0);
                self.process_message(&mut agent, request).await?;
                // process_message handles its own draw calls for streaming
                continue;
            }

            // Block until we get an event - no polling when idle
            if event::poll(std::time::Duration::from_secs(60))? {
                let needs_redraw = match event::read()? {
                    Event::Key(key) => {
                        if let Some(action) = map_key(InputMode::Normal, key) {
                            self.handle_action(action);
                        }
                        true
                    }
                    Event::Mouse(mouse) => {
                        if let Some(action) = map_mouse(mouse) {
                            self.handle_action(action);
                        }
                        true
                    }
                    Event::Resize(_, _) => true,
                    _ => false,
                };

                if needs_redraw {
                    self.draw()?;
                }
            }

            if self.should_quit {
                break;
            }
        }

        self.cleanup()
    }

    /// Draw the UI with synchronized updates to prevent tearing
    fn draw(&mut self) -> Result<()> {
        use ratatui::style::{Color, Style};
        use ratatui::widgets::Paragraph;
        
        self.last_render = Instant::now();

        let chat_widget = self.chat.widget(&mut self.transcript);
        let input_widget = self.input.widget(
            &self.config.general.model,
            self.usage.context_tokens,
        );
        let alert = self.alert.clone();

        // Calculate input height based on content, with min 3 and max half screen
        let input_height = self.input.required_height(self.terminal.size()?.width);
        let max_input_height = self.terminal.size()?.height / 2;
        let input_height = input_height.min(max_input_height).max(5);

        // Begin synchronized update - terminal buffers all changes
        queue!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;

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

        // End synchronized update - terminal renders atomically
        queue!(self.terminal.backend_mut(), EndSynchronizedUpdate)?;
        self.terminal.backend_mut().flush()?;

        Ok(())
    }

    /// Draw with frame rate limiting - skips if called too frequently
    /// Returns true if a draw actually occurred
    fn draw_throttled(&mut self) -> Result<bool> {
        if self.last_render.elapsed() >= MIN_FRAME_TIME {
            self.draw()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Handle an action. Returns true if the action should interrupt/break the current loop.
    fn handle_action(&mut self, action: Action) -> bool {
        // Interrupt actions - break out of streaming loop
        match action {
            Action::Interrupt => return true,
            Action::Quit => {
                self.should_quit = true;
                return true;
            }
            _ => {}
        }

        // Clear alert on any input action
        self.alert = None;
        
        // Common actions - safe in all modes
        match action {
            Action::InsertChar(c) => self.input.insert_char(c),
            Action::InsertNewline => self.input.insert_newline(),
            Action::DeleteBack => self.input.delete_char(),
            Action::DeleteForward => self.input.delete_char_forward(),
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
                if self.input.content().is_empty() {
                    self.input.history_prev();
                } else {
                    self.chat.scroll_up();
                }
            }
            Action::HistoryNext => {
                if self.input.content().is_empty() {
                    self.input.history_next();
                } else {
                    self.chat.scroll_down();
                }
            }
            Action::ScrollUp => self.chat.scroll_up(),
            Action::ScrollDown => self.chat.scroll_down(),
            Action::PageUp => self.chat.page_up(10),
            Action::PageDown => self.chat.page_down(10),
            Action::TabComplete => {
                if let Some(completed) = crate::commands::complete(self.input.content()) {
                    self.input.set_content(&completed);
                }
            }
            // These are handled in specific contexts or already matched above
            Action::Quit | Action::Interrupt | Action::ApproveTool | Action::DenyTool | Action::ApproveToolSession => {}
        }

        false
    }

    /// Wait for user to approve or deny a tool request
    async fn wait_for_tool_approval(&mut self) -> Result<ToolDecision> {
        // Drain any buffered key events first to prevent accidental approvals
        while event::poll(std::time::Duration::from_millis(0))? {
            let _ = event::read()?;
        }

        loop {
            if !event::poll(std::time::Duration::from_millis(50))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };

            if let Some(action) = map_key(InputMode::ToolApproval, key) {
                match action {
                    Action::ApproveTool => return Ok(ToolDecision::Approve),
                    Action::DenyTool => return Ok(ToolDecision::Deny),
                    Action::ApproveToolSession => {
                        // TODO: implement allow for session
                        return Ok(ToolDecision::Approve);
                    }
                    Action::Quit => {
                        self.should_quit = true;
                        return Ok(ToolDecision::Deny);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Poll for events without blocking.
    /// Returns (interrupted, needs_redraw)
    fn poll_events(&mut self, mode: InputMode) -> PollResult {
        if !event::poll(Duration::from_millis(0)).unwrap_or(false) {
            return PollResult::NoEvent;
        }

        match event::read() {
            Ok(Event::Key(key)) => {
                if let Some(action) = map_key(mode, key) {
                    if self.handle_action(action) {
                        PollResult::Interrupted
                    } else {
                        PollResult::Handled
                    }
                } else {
                    PollResult::NoEvent
                }
            }
            Ok(Event::Mouse(mouse)) => {
                if let Some(action) = map_mouse(mouse) {
                    self.handle_action(action);
                }
                PollResult::Handled
            }
            Ok(Event::Resize(_, _)) => PollResult::Handled,
            _ => PollResult::NoEvent,
        }
    }

    /// Queue a user message for sending
    fn queue_message(&mut self, content: String) {
        if let Some(command) = crate::commands::parse(&content) {
            let name = command.name().to_string();
            let turn_id = self.transcript.add_turn(
                Role::User, 
                TextBlock::pending(&format!("/{}", name))
            );
            self.message_queue.push(MessageRequest::Command(name, turn_id));
        } else {
            let turn_id = self.transcript.add_turn(Role::User, TextBlock::pending(&content));
            self.message_queue.push(MessageRequest::User(content, turn_id));
        }
        self.chat.enable_auto_scroll();
    }

    /// Queue a compaction request
    pub fn queue_compaction(&mut self) {
        self.message_queue.push(MessageRequest::Compaction);
        self.chat.enable_auto_scroll();
    }

    /// Process a message request (user message or compaction)
    async fn process_message(&mut self, agent: &mut Agent, request: MessageRequest) -> Result<()> {
        if let Err(e) = agent.refresh_oauth_if_needed().await {
            tracing::warn!("Failed to refresh OAuth token: {}", e);
        }

        match request {
            MessageRequest::User(content, turn_id) => {
                // Mark the user turn as complete (it was created as Pending in queue_message)
                if let Some(turn) = self.transcript.get_mut(turn_id) {
                    if let Some(block) = turn.content.first_mut() {
                        block.set_status(Status::Complete);
                    }
                }

                self.stream_response(agent, &content, RequestMode::Normal).await?;

                if agent.context_tokens() >= self.config.general.compaction_threshold {
                    self.queue_compaction();
                }
            }
            MessageRequest::Compaction => {
                // Stream assistant's compaction summary
                self.stream_response(agent, COMPACTION_PROMPT, RequestMode::Compaction).await?;
            }
            MessageRequest::Command(command, turn_id) => {
                // Mark the command turn as complete (it was created as Pending in queue_message)
                if let Some(turn) = self.transcript.get_mut(turn_id) {
                    if let Some(block) = turn.content.first_mut() {
                        block.set_status(Status::Complete);
                    }
                }

                if let Some(cmd) = crate::commands::get(&command) {
                    if let Err(e) = cmd.execute(self, agent) {
                        self.alert = Some(format!("Command error: {}", e));
                    }
                }
            }
        }

        Ok(())
    }

    /// Stream a response from the agent with a specific request mode
    async fn stream_response(
        &mut self,
        agent: &mut Agent,
        prompt: &str,
        mode: RequestMode,
    ) -> Result<()> {
        let mut stream = agent.process_message(prompt, mode);

        loop {
            // Poll for user input on every iteration (non-blocking)
            match self.poll_events(InputMode::Streaming) {
                PollResult::Interrupted => break,
                PollResult::Handled => { self.draw_throttled()?; }
                PollResult::NoEvent => {}
            }

            let step = match tokio::time::timeout(Duration::from_millis(100), stream.next()).await {
                Ok(Some(s)) => s,
                Ok(None) => break,
                Err(_) => continue,
            };

            match step {
                AgentStep::TextDelta(text) => {
                    let turn = self.transcript.get_or_create_current_turn();
                    if turn.is_active_block_type(BlockType::Text) {
                        turn.append_to_active(&text);
                    } else {
                        turn.start_block(Box::new(TextBlock::new(&text)));
                    }
                }
                AgentStep::CompactionDelta(text) => {
                    let turn = self.transcript.get_or_create_current_turn();
                    if turn.is_active_block_type(BlockType::Compaction) {
                        turn.append_to_active(&text);
                    } else {
                        turn.start_block(Box::new(CompactionBlock::new(&text)));
                    }
                }
                AgentStep::ThinkingDelta(text) => {
                    let turn = self.transcript.get_or_create_current_turn();
                    if turn.is_active_block_type(BlockType::Thinking) {
                        turn.append_to_active(&text);
                    } else {
                        turn.start_block(Box::new(ThinkingBlock::new(&text)));
                    }
                }
                AgentStep::ToolRequest { call_id, name, params } => {
                    // Get tool info via stream.agent (stream holds mutable borrow)
                    let tool = stream.agent.get_tool(&name);
                    let post_actions = tool.post_actions(&params);

                    self.transcript.get_or_create_current_turn()
                        .start_block(tool.create_block(&call_id, params.clone()));
                    self.draw()?;

                    // Show preview in IDE if the tool provides one
                    if let Some(preview) = tool.preview(&params) {
                        if let Some(ide) = &self.ide {
                            if let Err(e) = ide.show_preview(&preview).await {
                                tracing::warn!("Failed to show IDE preview: {}", e);
                            }
                        }
                    }

                    // Determine decision based on filter result or user approval
                    let decision = match self.tool_filters.evaluate(&name, &params) {
                        Some(decision) => decision,
                        None => self.wait_for_tool_approval().await?,
                    };

                    // Close preview after decision is made
                    if let Some(ide) = &self.ide {
                        if let Err(e) = ide.close_preview().await {
                            tracing::warn!("Failed to close IDE preview: {}", e);
                        }
                    }

                    if let Some(block) = self.transcript.get_or_create_current_turn().get_active_block_mut() {
                        match decision {
                            ToolDecision::Approve => block.set_status(Status::Running),
                            ToolDecision::Deny => block.set_status(Status::Denied),
                        }
                    }
                    self.draw()?;

                    let tool_result = stream.decide_tool(decision).await;
                    if let Some(AgentStep::ToolResult { result, is_error, .. }) = tool_result {
                        if let Some(block) = self.transcript.get_or_create_current_turn().get_active_block_mut() {
                            block.set_status(if is_error { Status::Error } else { Status::Complete });
                            block.append_text(&result);
                        }

                        // Execute post-actions from the tool (e.g., reload buffer)
                        if !is_error {
                            for action in post_actions {
                                if let Some(ide) = &self.ide {
                                    if let Err(e) = ide.execute(&action).await {
                                        tracing::warn!("Failed to execute IDE action: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                AgentStep::ToolResult { .. } => {}
                AgentStep::Retrying { attempt, error } => {
                    self.status = ConnectionStatus::Error(format!("Retry {} - {}", attempt, error));
                }
                AgentStep::Finished { usage, thinking_signatures: _ } => {
                    self.usage = usage;
                    self.status = ConnectionStatus::Connected;

                    let turn = self.transcript.get_or_create_current_turn();

                    // Mark active block complete
                    if let Some(block) = turn.get_active_block_mut() {
                        block.set_status(Status::Complete);
                    }

                    // Save and rotate transcript for compaction (agent handles its own reset)
                    if turn.is_active_block_type(BlockType::Compaction) {
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
                    }
                }
                AgentStep::Error(msg) => {
                    // Mark active block as complete (or error) if there is one
                    let turn = self.transcript.get_or_create_current_turn();
                    if let Some(block) = turn.get_active_block_mut() {
                        block.set_status(Status::Error);
                    }

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
            
            // Throttled draw at end of loop to show streaming updates
            self.draw_throttled()?;
        }

        self.chat.enable_auto_scroll();
        self.draw()?;

        // Clear current turn now that streaming is complete
        self.transcript.clear_current_turn();

        if let Err(e) = self.transcript.save() {
            tracing::error!("Failed to save transcript: {}", e);
        }

        Ok(())
    }

    /// Cleanup terminal
    fn cleanup(&mut self) -> Result<()> {
        disable_raw_mode().context("Failed to disable raw mode")?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
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


