use crate::config::Config;
use crate::llm::{Agent, AgentStep, ToolDecision, Usage};
use crate::transcript::{Role, Status, TextBlock, Transcript, TurnId};
use crate::tools::ToolRegistry;
use crate::ui::{ChatView, ConnectionStatus, InputBox};

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
use std::path::PathBuf;
use std::time::{Duration, Instant};

const APP_NAME: &str = "Codey";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const TRANSCRIPT_DIR: &str = ".codey";
const TRANSCRIPT_FILE: &str = "transcript.json";

/// Minimum time between frames (~60fps)
const MIN_FRAME_TIME: Duration = Duration::from_millis(16);

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

### Editing Files
- Use `edit_file` for existing files, `write_file` only for new files
- The `old_string` must match EXACTLY, including whitespace and indentation
- If `old_string` appears multiple times, include more context to make it unique
- Apply edits sequentially; each edit sees the result of previous edits

### Shell Commands
- Prefer `read_file` over `cat`, `head`, `tail`
- Use `ls` for directory exploration
- Use `grep` or `rg` for searching code
- Always use absolute paths or paths relative to working directory

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
    transcript_path: PathBuf,
    chat: ChatView,
    input: InputBox,
    status: ConnectionStatus,
    usage: Usage,
    should_quit: bool,
    continue_session: bool,
    /// Queue of messages waiting to be sent (content, message_id)
    message_queue: Vec<(String, TurnId)>,
    /// Last render time for frame rate limiting
    last_render: Instant,
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

        // Set up transcript path in current working directory
        let transcript_path = PathBuf::from(TRANSCRIPT_DIR).join(TRANSCRIPT_FILE);

        // Load existing transcript if continuing
        let transcript = if continue_session {
            Transcript::load(&transcript_path).unwrap_or_default()
        } else {
            Transcript::new()
        };

        Ok(Self {
            config,
            terminal,
            transcript,
            transcript_path,
            chat: ChatView::new(),
            input: InputBox::new(),
            status: ConnectionStatus::Disconnected,
            usage: Usage::default(),
            should_quit: false,
            continue_session,
            message_queue: Vec::new(),
            last_render: Instant::now(),
        })
    }

    /// Run the main event loop - purely event-driven rendering
    pub async fn run(&mut self) -> Result<()> {
        let tools = ToolRegistry::new();
        let messages = vec![(Role::System, SYSTEM_PROMPT.to_string())];
        let mut agent = Agent::new(
            &self.config.general.model,
            self.config.general.max_tokens,
            self.config.general.max_retries,
            messages,
            tools,
        );

        // Restore agent context if continuing session
        if self.continue_session && !self.transcript.turns().is_empty() {
            agent.restore_from_transcript(&self.transcript);
        } else {
            // Show welcome message only for new sessions
            self.transcript.add(
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
            if let Some((content, msg_id)) = self.message_queue.first().cloned() {
                self.message_queue.remove(0);
                self.send_message(&mut agent, content, msg_id).await?;
                // send_message handles its own draw calls for streaming
                continue;
            }

            // Block until we get an event - no polling when idle
            // This is the key optimization: we don't spin or redraw when nothing happens
            if event::poll(std::time::Duration::from_secs(60))? {
                let needs_redraw = match event::read()? {
                    Event::Key(key) => {
                        self.handle_key_event(key);
                        true
                    }
                    Event::Mouse(mouse) => {
                        self.handle_mouse_event(mouse);
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
        self.last_render = Instant::now();

        let chat_widget = self.chat.widget(&self.transcript);
        let input_widget = self.input.widget(&self.config.general.model);

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
                    Constraint::Min(5),              // Chat area (minimum)
                    Constraint::Length(input_height), // Input area (dynamic)
                ])
                .split(frame.area());

            frame.render_widget(chat_widget, chunks[0]);
            frame.render_widget(input_widget, chunks[1]);
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

    /// Handle a key event
    fn handle_key_event(&mut self, key: KeyEvent) {
        // Global shortcuts
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('l') => {
                    self.transcript.clear();
                    return;
                }
                KeyCode::Up => {
                    self.chat.scroll_up();
                    return;
                }
                KeyCode::Down => {
                    self.chat.scroll_down();
                    return;
                }
                _ => {}
            }
        }

        // Input handling - always enabled now
        match key.code {
            KeyCode::Char(c) => {
                self.input.insert_char(c);
            }
            KeyCode::Backspace => {
                self.input.delete_char();
            }
            KeyCode::Delete => {
                self.input.delete_char_forward();
            }
            KeyCode::Left => {
                self.input.move_cursor_left();
            }
            KeyCode::Right => {
                self.input.move_cursor_right();
            }
            KeyCode::Home => {
                self.input.move_cursor_start();
            }
            KeyCode::End => {
                self.input.move_cursor_end();
            }
            KeyCode::Enter => {
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT)
                {
                    // Shift+Enter or Alt+Enter inserts newline
                    self.input.insert_newline();
                } else {
                    // Enter queues message
                    let content = self.input.submit();
                    if !content.trim().is_empty() {
                        self.queue_message(content);
                    }
                }
            }
            KeyCode::Esc => {
                self.input.clear();
            }
            KeyCode::Up => {
                if self.input.content().is_empty() {
                    self.input.history_prev();
                } else {
                    self.chat.scroll_up();
                }
            }
            KeyCode::Down => {
                if self.input.content().is_empty() {
                    self.input.history_next();
                } else {
                    self.chat.scroll_down();
                }
            }
            KeyCode::PageUp => {
                self.chat.page_up(10);
            }
            KeyCode::PageDown => {
                self.chat.page_down(10);
            }
            _ => {}
        }
    }

    /// Handle mouse events
    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.chat.scroll_up();
                self.chat.scroll_up();
                self.chat.scroll_up();
            }
            MouseEventKind::ScrollDown => {
                self.chat.scroll_down();
                self.chat.scroll_down();
                self.chat.scroll_down();
            }
            _ => {}
        }
    }

    /// Queue a message for sending
    fn queue_message(&mut self, content: String) {
        // Add to transcript and mark as pending
        let id = self.transcript.add(Role::User, TextBlock::new(&content));
        if let Some(turn) = self.transcript.get_mut(id) {
            turn.status = Status::Pending;
        }
        // Add to queue with turn ID
        self.message_queue.push((content, id));
        // Scroll to show new message
        self.chat.enable_auto_scroll();
    }

    /// Send a message to the agent
    async fn send_message(&mut self, agent: &mut Agent, content: String, user_turn_id: TurnId) -> Result<()> {
        // Mark user turn as running
        if let Some(turn) = self.transcript.get_mut(user_turn_id) {
            turn.status = Status::Running;
        }
        self.draw()?;

        // Track the current assistant turn being built
        let mut current_turn_id: Option<TurnId> = None;
        let mut streaming_block_idx: Option<usize> = None;

        // Create the stream - agent is borrowed mutably for its lifetime
        let mut stream = agent.process_message(&content);

        // Mark user turn as sent
        if let Some(turn) = self.transcript.get_mut(user_turn_id) {
            turn.status = Status::Success;
        }
        self.draw()?;

        loop {
            let step = match stream.next().await {
                Some(s) => s,
                None => break,
            };

            match step {
                AgentStep::TextDelta(text) => {
                    // Create turn on first chunk, append on subsequent
                    if current_turn_id.is_none() {
                        current_turn_id = Some(self.transcript.add_empty(Role::Assistant));
                    }
                    if let Some(turn) = self.transcript.get_mut(current_turn_id.unwrap()) {
                        if streaming_block_idx.is_none() {
                            streaming_block_idx = Some(turn.add_text_block(&text));
                        } else {
                            turn.append_to_block(streaming_block_idx.unwrap(), &text);
                        }
                    }
                    // Use throttled draw for streaming - caps at 60fps
                    self.draw_throttled()?;
                }
                AgentStep::ToolRequest { call_id, block, .. } => {
                    // Reset streaming block - next text will be a new block
                    streaming_block_idx = None;
                    
                    // Add tool block to turn
                    if current_turn_id.is_none() {
                        current_turn_id = Some(self.transcript.add_boxed(Role::Assistant, block));
                    } else if let Some(turn) = self.transcript.get_mut(current_turn_id.unwrap()) {
                        turn.add_block(block);
                    }

                    self.draw()?;

                    // Wait for user approval
                    let decision = self.wait_for_tool_approval().await?;

                    // Update tool status based on decision
                    if let Some(turn) = current_turn_id.and_then(|id| self.transcript.get_mut(id)) {
                        if let Some(tool) = turn.get_block_mut(&call_id) {
                            match decision {
                                ToolDecision::Approve => tool.set_status(Status::Running),
                                ToolDecision::Deny => tool.set_status(Status::Denied),
                            }
                        }
                    }
                    self.draw()?;

                    // Tell agent what to do and get the result
                    if let Some(AgentStep::ToolResult {
                        call_id,
                        result,
                        is_error,
                        ..
                    }) = stream.decide_tool(decision).await
                    {
                        if let Some(turn) = current_turn_id.and_then(|id| self.transcript.get_mut(id))
                        {
                            if let Some(tool) = turn.get_block_mut(&call_id) {
                                tool.set_status(if is_error { Status::Error } else { Status::Success });
                                tool.set_result(result);
                            }
                        }
                        self.draw()?;
                    }
                }
                AgentStep::ToolResult { .. } => {
                    // Handled inline after decide_tool
                }
                AgentStep::Retrying { attempt, error } => {
                    self.status =
                        ConnectionStatus::Error(format!("Retry {} - {}", attempt, error));
                    self.draw()?;
                }
                AgentStep::Finished { usage } => {
                    self.usage = usage;
                    self.status = ConnectionStatus::Connected;
                }
                AgentStep::Error(msg) => {
                    self.status = ConnectionStatus::Error(msg);
                }
            }
        }

        self.chat.enable_auto_scroll();

        // Final draw to ensure complete state is rendered
        // (throttled draws during streaming may have skipped the last few deltas)
        self.draw()?;

        // Auto-save transcript after turn completes
        if let Err(e) = self.transcript.save(&self.transcript_path) {
            tracing::error!("Failed to save transcript: {}", e);
        }

        Ok(())
    }

    /// Wait for user to approve or deny a tool request
    async fn wait_for_tool_approval(&mut self) -> Result<ToolDecision> {
        // Drain any buffered key events first to prevent accidental approvals
        while event::poll(std::time::Duration::from_millis(0))? {
            let _ = event::read()?;
        }

        loop {
            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            return Ok(ToolDecision::Approve);
                        }
                        KeyCode::Char('n') | KeyCode::Esc => {
                            return Ok(ToolDecision::Deny);
                        }
                        KeyCode::Char('a') => {
                            // TODO: implement allow for session
                            return Ok(ToolDecision::Approve);
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.should_quit = true;
                            return Ok(ToolDecision::Deny);
                        }
                        _ => {}
                    }
                }
            }
        }
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


