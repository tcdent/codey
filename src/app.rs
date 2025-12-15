use crate::config::Config;
use crate::llm::{Agent, AgentStep, ToolDecision, Usage};
use crate::transcript::{Role, Status, TextBlock, ThinkingBlock, Transcript, TurnId};
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
const CODEY_DIR: &str = ".codey";
const TRANSCRIPTS_DIR: &str = "transcripts";
const MIN_FRAME_TIME: Duration = Duration::from_millis(16);

/// Get the transcripts directory path, creating it if necessary
fn get_transcripts_dir() -> Result<PathBuf> {
    let dir = PathBuf::from(CODEY_DIR).join(TRANSCRIPTS_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).context("Failed to create transcripts directory")?;
    }
    Ok(dir)
}

/// Find the latest transcript number by scanning the transcripts directory
fn find_latest_transcript_number(dir: &PathBuf) -> Option<u32> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.ends_with(".json") {
                name.trim_end_matches(".json").parse::<u32>().ok()
            } else {
                None
            }
        })
        .max()
}

/// Get the path for a transcript with a given number
fn transcript_path(dir: &PathBuf, number: u32) -> PathBuf {
    dir.join(format!("{:06}.json", number))
}

/// Tracks the currently active block during streaming
enum ActiveBlock {
    None,
    Text(usize),
    Thinking(usize),
    Tool(usize),
}

/// Input modes determine which keybindings are active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Normal,
    Streaming,
    ToolApproval,
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
    ClearTranscript,
    Interrupt,
    Quit,
    ApproveTool,
    DenyTool,
    ApproveToolSession,
}

/// Map a key event to an action based on the current input mode
fn map_key(mode: InputMode, key: KeyEvent) -> Option<Action> {
    // Global shortcuts (work in all modes)
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Some(Action::Quit),
            _ => {}
        }
    }

    match mode {
        InputMode::Normal => map_key_normal(key),
        InputMode::Streaming => map_key_streaming(key),
        InputMode::ToolApproval => map_key_tool_approval(key),
    }
}

fn map_key_normal(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('l') => Some(Action::ClearTranscript),
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
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) 
                      || key.modifiers.contains(KeyModifiers::ALT) => Some(Action::InsertNewline),
        KeyCode::Enter => Some(Action::Submit),
        KeyCode::Esc => Some(Action::ClearInput),
        KeyCode::Up => Some(Action::HistoryPrev),
        KeyCode::Down => Some(Action::HistoryNext),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),
        _ => None,
    }
}

fn map_key_streaming(key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::Interrupt),
        _ => None,
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
    /// Alert message to display (cleared on next user input)
    alert: Option<String>,
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
        let transcripts_dir = get_transcripts_dir()?;
        let latest_number = find_latest_transcript_number(&transcripts_dir);
        
        let (transcript_path, transcript) = if continue_session {
            // Continue from latest transcript if it exists
            match latest_number {
                Some(n) => {
                    let path = transcript_path(&transcripts_dir, n);
                    let transcript = Transcript::load(&path).unwrap_or_default();
                    (path, transcript)
                }
                None => {
                    // No existing transcripts, start fresh at 000000
                    (transcript_path(&transcripts_dir, 0), Transcript::new())
                }
            }
        } else {
            // Start new session with next number
            let next_number = latest_number.map(|n| n + 1).unwrap_or(0);
            (transcript_path(&transcripts_dir, next_number), Transcript::new())
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
            alert: None,
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
        
        let messages = vec![(Role::System, SYSTEM_PROMPT.to_string())];
        
        let mut agent = Agent::new(
            &self.config.general.model,
            self.config.general.max_tokens,
            self.config.general.max_retries,
            messages,
            tools,
            oauth,
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
                Status::Complete,
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
            if event::poll(std::time::Duration::from_secs(60))? {
                let needs_redraw = match event::read()? {
                    Event::Key(key) => {
                        if let Some(action) = map_key(InputMode::Normal, key) {
                            self.handle_action(action);
                        }
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
        use ratatui::style::{Color, Style};
        use ratatui::widgets::Paragraph;
        
        self.last_render = Instant::now();

        let chat_widget = self.chat.widget(&self.transcript);
        let input_widget = self.input.widget(&self.config.general.model);
        let alert = self.alert.clone();

        // Calculate input height based on content, with min 3 and max half screen
        let input_height = self.input.required_height(self.terminal.size()?.width);
        let max_input_height = self.terminal.size()?.height / 2;
        let input_height = input_height.min(max_input_height).max(5);
        
        let alert_height = if alert.is_some() { 1 } else { 0 };

        // Begin synchronized update - terminal buffers all changes
        queue!(self.terminal.backend_mut(), BeginSynchronizedUpdate)?;

        self.terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(5),               // Chat area (minimum)
                    Constraint::Length(input_height), // Input area (dynamic)
                    Constraint::Length(alert_height), // Alert bar (0 or 1)
                ])
                .split(frame.area());

            frame.render_widget(chat_widget, chunks[0]);
            frame.render_widget(input_widget, chunks[1]);
            
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

    /// Handle an action
    fn handle_action(&mut self, action: Action) {
        // Clear alert on any input action
        self.alert = None;
        
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
            Action::ClearTranscript => self.transcript.clear(),
            Action::Quit => self.should_quit = true,
            // These are handled in specific contexts
            Action::Interrupt | Action::ApproveTool | Action::DenyTool | Action::ApproveToolSession => {}
        }
    }

    /// Handle mouse events
    fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.chat.scroll_up();
            }
            MouseEventKind::ScrollDown => {
                self.chat.scroll_down();
            }
            _ => {}
        }
    }

    /// Check for interrupt keys without blocking
    /// Returns true if an interrupt was requested
    fn check_for_interrupt(&mut self) -> bool {
        if !event::poll(Duration::from_millis(0)).unwrap_or(false) {
            return false;
        }
        let Ok(Event::Key(key)) = event::read() else {
            return false;
        };
        let Some(action) = map_key(InputMode::Streaming, key) else {
            return false;
        };
        match action {
            Action::Interrupt => true,
            Action::Quit => {
                self.should_quit = true;
                true
            }
            _ => false,
        }
    }

    /// Queue a message for sending
    fn queue_message(&mut self, content: String) {
        let id = self.transcript.add(Role::User, TextBlock::new(&content), Status::Pending);
        self.message_queue.push((content, id));
        self.chat.enable_auto_scroll();
    }

    /// Send a message to the agent
    async fn send_message(&mut self, agent: &mut Agent, content: String, user_turn_id: TurnId) -> Result<()> {
        // Mark user turn as running
        if let Some(turn) = self.transcript.get_mut(user_turn_id) {
            turn.status = Status::Running;
        }
        self.draw()?;

        // Refresh OAuth token if needed
        if let Err(e) = agent.refresh_oauth_if_needed().await {
            tracing::warn!("Failed to refresh OAuth token: {}", e);
        }

        // Track the current assistant turn and active block
        let mut current_turn_id: Option<TurnId> = None;
        let mut active_block = ActiveBlock::None;

        // Create the stream - agent is borrowed mutably for its lifetime
        let mut stream = agent.process_message(&content);

        // Mark user turn as sent
        if let Some(turn) = self.transcript.get_mut(user_turn_id) {
            turn.status = Status::Complete;
        }
        self.draw()?;

        loop {
            // Check for interrupt before each step
            if self.check_for_interrupt() {
                if let Some(turn_id) = current_turn_id {
                    if let Some(turn) = self.transcript.get_mut(turn_id) {
                        turn.status = Status::Cancelled;
                    }
                }
                self.draw()?;
                break;
            }

            // Use timeout on stream.next() to allow periodic interrupt checks
            let step = match tokio::time::timeout(Duration::from_millis(100), stream.next()).await {
                Ok(Some(s)) => {
                    tracing::debug!("Stream step: {:?}", std::mem::discriminant(&s));
                    s
                }
                Ok(None) => {
                    tracing::debug!("Stream ended (None)");
                    break;
                }
                Err(_) => continue, // Timeout, go back to interrupt check
            };

            match step {
                AgentStep::TextDelta(text) => {
                    let turn_id = *current_turn_id.get_or_insert_with(|| {
                        self.transcript.add_empty(Role::Assistant, Status::Running)
                    });
                    if let Some(turn) = self.transcript.get_mut(turn_id) {
                        match active_block {
                            ActiveBlock::Text(idx) => turn.append_to_block(idx, &text),
                            _ => active_block = ActiveBlock::Text(turn.add_block(Box::new(TextBlock::new(&text)))),
                        }
                    }
                    self.draw_throttled()?;
                }
                AgentStep::ThinkingDelta(text) => {
                    let turn_id = *current_turn_id.get_or_insert_with(|| {
                        self.transcript.add_empty(Role::Assistant, Status::Running)
                    });
                    if let Some(turn) = self.transcript.get_mut(turn_id) {
                        match active_block {
                            ActiveBlock::Thinking(idx) => turn.append_to_block(idx, &text),
                            _ => active_block = ActiveBlock::Thinking(turn.add_block(Box::new(ThinkingBlock::new(&text, "")))),
                        }
                    }
                    self.draw()?;
                }
                AgentStep::ToolRequest { block, .. } => {
                    let turn_id = *current_turn_id.get_or_insert_with(|| {
                        self.transcript.add_empty(Role::Assistant, Status::Running)
                    });
                    if let Some(turn) = self.transcript.get_mut(turn_id) {
                        active_block = ActiveBlock::Tool(turn.add_block(block));
                    }

                    self.draw()?;

                    // Wait for user approval
                    let decision = self.wait_for_tool_approval().await?;

                    // Update tool status based on decision
                    if let ActiveBlock::Tool(idx) = active_block {
                        if let Some(turn) = current_turn_id.and_then(|id| self.transcript.get_mut(id)) {
                            if let Some(tool) = turn.get_block_mut(idx) {
                                match decision {
                                    ToolDecision::Approve => tool.set_status(Status::Running),
                                    ToolDecision::Deny => tool.set_status(Status::Denied),
                                }
                            }
                        }
                    }
                    self.draw()?;

                    // Tell agent what to do and get the result
                    let tool_result = stream.decide_tool(decision).await;
                    tracing::debug!("decide_tool returned: {:?}", tool_result.as_ref().map(|s| std::mem::discriminant(s)));
                    if let Some(AgentStep::ToolResult {
                        result,
                        is_error,
                        ..
                    }) = tool_result
                    {
                        if let ActiveBlock::Tool(idx) = active_block {
                            if let Some(turn) = current_turn_id.and_then(|id| self.transcript.get_mut(id)) {
                                if let Some(tool) = turn.get_block_mut(idx) {
                                    tool.set_status(if is_error { Status::Error } else { Status::Complete });
                                    tool.set_result(result);
                                }
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
                AgentStep::Finished { usage, thinking_signatures } => {
                    self.usage = usage;
                    self.status = ConnectionStatus::Connected;
                    
                    // Update thinking blocks with their signatures
                    if let Some(turn_id) = current_turn_id {
                        if let Some(turn) = self.transcript.get_mut(turn_id) {
                            let mut sig_iter = thinking_signatures.into_iter();
                            for block in &mut turn.content {
                                // If this block has a signature field (is a ThinkingBlock)
                                if block.signature().is_some() {
                                    if let Some(sig) = sig_iter.next() {
                                        block.set_signature(&sig);
                                    }
                                }
                            }
                        }
                    }
                }
                AgentStep::Error(msg) => {
                    // Try to parse API error JSON to extract message
                    let alert_msg = if let Some(start) = msg.find('{') {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg[start..]) {
                            json["error"]["message"]
                                .as_str()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| msg.clone())
                        } else {
                            msg.clone()
                        }
                    } else {
                        msg.clone()
                    };
                    self.alert = Some(alert_msg);
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


