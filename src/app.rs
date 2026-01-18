use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal, TerminalOptions, Viewport,
};
use tokio::sync::oneshot;

use crate::commands::Command;
use crate::config::{AgentRuntimeConfig, Config};
use crate::ide::{Ide, IdeEvent, Nvim};
use crate::llm::{Agent, AgentId, AgentRegistry, AgentStep, RequestMode};
#[cfg(feature = "profiling")]
use crate::{profile_frame, profile_span};
use crate::tool_filter::ToolFilters;
use crate::tools::{
    init_agent_context, update_agent_oauth, Effect, ToolCall, ToolDecision, ToolEvent,
    ToolExecutor, ToolRegistry,
};
use crate::transcript::{BlockType, Role, Status, TextBlock, Transcript};
use crate::ui::{Attachment, ChatView, InputBox};

const MIN_FRAME_TIME: Duration = Duration::from_millis(16);

pub const APP_NAME: &str = "Codey";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CODEY_DIR: &str = ".codey";
pub const TRANSCRIPTS_DIR: &str = "transcripts";

const WELCOME_MESSAGE: &str =
    "Welcome to Codey! I'm your AI coding assistant. How can I help you today?";
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
- You can do multiple edits at once, but keep it under 1000 lines
- Avoid the urge to completely rewrite files - make precise, minimal edits so the user can review them easily

### Shell Commands
- Prefer `read_file` over `cat`, `head`, `tail`
- Use `ls` for directory exploration
- Use `grep` or `rg` for searching code

### Background Execution
For long-running operations, you can execute tools in the background by adding `"background": true` to the tool call. This returns immediately with a task ID while the tool runs asynchronously.
Use `list_background_tasks` to check status and `get_background_task` to retrieve results when complete.

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
6. **Quotes and log snippets** - Any important quotes or logs that the user provided that we'll need later

Be thorough but concise - this summary will seed a fresh conversation context."#;

pub const SUB_AGENT_PROMPT: &str = r#"You are a background research agent. Your task is to investigate, explore, or analyze as directed.

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
    // Only handle key press events, not release or repeat
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match mode {
        InputMode::Normal => map_key_normal(key),
        InputMode::Streaming => map_key_streaming(key),
        InputMode::ToolApproval => map_key_tool_approval(key),
    }
}

/// Keybindings for normal input mode
fn map_key_normal(key: KeyEvent) -> Option<Action> {
    // With REPORT_ALTERNATE_KEYS, crossterm gives us the shifted character directly
    // (e.g., '!' instead of '1' with SHIFT) and clears the SHIFT modifier.
    // We only need to check modifiers for special key combos.
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
    /// Queue of tools awaiting approval - front is currently displayed
    pending_approvals: VecDeque<(ToolCall, oneshot::Sender<ToolDecision>)>,
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
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | crossterm::event::KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
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
            },
        )
        .context("Failed to create terminal")?;

        // Load existing transcript or create new one
        let transcript = if continue_session {
            Transcript::load().context("Failed to load transcript")?
        } else {
            Transcript::new_numbered().context("Failed to create new transcript")?
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
                },
                Ok(None) => {
                    tracing::debug!("No nvim instance found");
                    None
                },
                Err(e) => {
                    tracing::warn!("Failed to connect to nvim: {}", e);
                    None
                },
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
            pending_approvals: VecDeque::new(),
        })
    }

    /// Run the main event loop - purely event-driven rendering
    pub async fn run(&mut self) -> Result<()> {
        self.oauth = crate::auth::OAuthCredentials::load().ok().flatten();

        // Initialize agent context for sub-agent spawning
        init_agent_context(
            AgentRuntimeConfig::background(&self.config),
            self.oauth.clone(),
        );

        let mut agent = Agent::new(
            AgentRuntimeConfig::foreground(&self.config),
            SYSTEM_PROMPT,
            self.oauth.clone(),
            self.tool_executor.tools().clone(),
        );

        if self.continue_session {
            agent.restore_from_transcript(&self.chat.transcript);
        } else {
            self.chat
                .add_turn(Role::Assistant, TextBlock::pending(WELCOME_MESSAGE));
        }
        self.agents.register(agent);

        // Initial render - populate hot zone from transcript
        self.chat.render(&mut self.terminal);
        self.draw();

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
    fn draw(&mut self) {
        #[cfg(feature = "profiling")]
        let _span = profile_span!("App::draw");
        #[cfg(feature = "profiling")]
        profile_frame!();

        use ratatui::style::{Color, Style};
        use ratatui::widgets::Paragraph;

        self.last_render = Instant::now();

        // Calculate dimensions
        let size = match self.terminal.size() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to get terminal size: {}", e);
                return;
            },
        };
        let input_height = self.input.required_height(size.width);
        let max_input_height = size.height / 2;
        let input_height = input_height.min(max_input_height).max(5);

        // Draw the viewport (hot zone content + input)
        let chat_widget = self.chat.widget();
        let context_tokens = self
            .agents
            .primary()
            .and_then(|m| m.try_lock().ok())
            .map_or(0, |a| a.total_usage().context_tokens);
        let input_widget = self
            .input
            .widget(
                &self.config.agents.foreground.model,
                context_tokens,
                self.tool_executor.running_background_count(),
            );
        let alert = self.alert.clone();

        if let Err(e) = self.terminal.draw(|frame| {
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
                let alert_widget =
                    Paragraph::new(msg.as_str()).style(Style::default().fg(Color::Red));
                frame.render_widget(alert_widget, chunks[2]);
            }
        }) {
            tracing::warn!("Failed to draw: {}", e);
        }
    }

    /// Draw with frame rate limiting - skips if called too frequently
    /// Returns true if a draw actually occurred
    fn draw_throttled(&mut self) -> bool {
        if self.last_render.elapsed() < MIN_FRAME_TIME {
            return false;
        }

        self.draw();
        true
    }

    /// Render chat and draw with frame rate limiting
    /// Use this for streaming updates where we get many deltas per second
    fn render_and_draw_throttled(&mut self) {
        if self.last_render.elapsed() < MIN_FRAME_TIME {
            return;
        }
        self.chat.render(&mut self.terminal);
        self.draw();
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
            },
            Action::ApproveTool => {
                self.decide_pending_tool(ToolDecision::Approve).await;
            },
            Action::DenyTool => {
                self.decide_pending_tool(ToolDecision::Deny).await;
            },
            Action::InsertChar(c) => self.input.insert_char(c),
            Action::InsertNewline => self.input.insert_newline(),
            Action::DeleteBack => self.input.delete_char(),
            Action::Paste(content) => {
                self.input.add_attachment(Attachment::pasted(content));
            },
            Action::CursorLeft => self.input.move_cursor_left(),
            Action::CursorRight => self.input.move_cursor_right(),
            Action::CursorHome => self.input.move_cursor_start(),
            Action::CursorEnd => self.input.move_cursor_end(),
            Action::Submit => {
                let content = self.input.submit();
                if !content.trim().is_empty() {
                    self.queue_message(content);
                }
            },
            Action::ClearInput => self.input.clear(),
            Action::HistoryPrev => {
                self.input.history_prev();
            },
            Action::HistoryNext => {
                self.input.history_next();
            },
            Action::TabComplete => {
                if let Some(completed) = Command::complete(&self.input.content()) {
                    self.input.set_content(&completed);
                }
            },
            Action::Resize(w, _h) => {
                // Update chat view width for text wrapping
                self.chat.set_width(w);
                // Re-render and draw with new dimensions
                self.chat.render(&mut self.terminal);
            },
        }

        ActionResult::Continue
    }

    /// Queue a user message or command for processing
    fn queue_message(&mut self, content: String) {
        let message = match Command::parse(&content) {
            Some(command) => {
                // Slash command
                let name = command.name().to_string();
                let turn_id = self
                    .chat
                    .add_turn(Role::User, TextBlock::pending(format!("/{}", name)));
                MessageRequest::Command(name, turn_id)
            },
            None => {
                // Regular user message
                let turn_id = self.chat.add_turn(Role::User, TextBlock::pending(&content));
                MessageRequest::User(content, turn_id)
            },
        };
        self.message_queue.push_back(message);

        self.chat.render(&mut self.terminal);
        self.draw();
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
        self.chat.finish_turn(&mut self.terminal);
        if let Err(e) = self.chat.transcript.save() {
            tracing::error!("Failed to save transcript on cancel: {}", e);
        }
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
            },
        };

        let Some(action) = map_event(self.input_mode, event) else {
            return Ok(());
        };

        match self.handle_action(action).await {
            ActionResult::Interrupt => {
                self.cancel().await?;
            },
            ActionResult::Continue => {
                self.draw_throttled();
            },
            ActionResult::NoOp => {},
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
            },
        }
    }

    /// Refresh OAuth credentials if expired, updating both App and primary agent
    async fn refresh_oauth(&mut self) {
        if let Some(ref oauth) = self.oauth {
            if oauth.is_expired() {
                tracing::info!("Refreshing expired OAuth token");
                match crate::auth::refresh_token(oauth).await {
                    Ok(new_creds) => {
                        if let Err(e) = new_creds.save() {
                            tracing::warn!("Failed to save refreshed OAuth credentials: {}", e);
                        }
                        // Update App's copy
                        self.oauth = Some(new_creds.clone());
                        // Update agent context for sub-agents
                        update_agent_oauth(self.oauth.clone()).await;
                        // Update primary agent's copy
                        if let Some(agent_mutex) = self.agents.primary() {
                            agent_mutex.lock().await.set_oauth(Some(new_creds));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to refresh OAuth token: {}", e);
                    }
                }
            }
        }
    }

    /// Start processing a message request
    async fn handle_message(&mut self, request: MessageRequest) -> Result<()> {
        // Refresh OAuth token if needed
        self.refresh_oauth().await;

        match request {
            MessageRequest::Command(name, turn_id) => {
                self.chat.mark_last_block_complete(turn_id);

                if let Some(command) = Command::get(&name) {
                    match command.execute(self) {
                        Ok(None) => {
                            // Command executed, no output - still need to render
                            self.chat.render(&mut self.terminal);
                            self.draw();
                        },
                        Ok(Some(output)) => {
                            let idx = self.chat.transcript.add_empty(Role::Assistant);
                            if let Some(turn) = self.chat.transcript.get_mut(idx) {
                                turn.start_block(Box::new(TextBlock::complete(&output)));
                            }
                            self.chat.render(&mut self.terminal);
                            self.draw();
                        },
                        Err(e) => {
                            tracing::error!("Command execution error: {}", e);
                            self.alert = Some(format!("Command error: {}", e));
                        },
                    }
                }
            },
            MessageRequest::User(content, turn_id) => {
                self.chat.mark_last_block_complete(turn_id);
                self.chat.render(&mut self.terminal);
                self.draw();

                if let Some(agent_mutex) = self.agents.primary() {
                    agent_mutex
                        .lock()
                        .await
                        .send_request(&content, RequestMode::Normal);
                }
                self.chat.begin_turn(Role::Assistant, &mut self.terminal);
                self.input_mode = InputMode::Streaming;
            },
            MessageRequest::Compaction => {
                if let Some(agent_mutex) = self.agents.primary() {
                    agent_mutex
                        .lock()
                        .await
                        .send_request(COMPACTION_PROMPT, RequestMode::Compaction);
                }
                self.chat.begin_turn(Role::Assistant, &mut self.terminal);
                self.input_mode = InputMode::Streaming;
            },
        }

        self.chat.render(&mut self.terminal);
        self.draw();

        Ok(())
    }

    /// Handle a single agent step during streaming
    async fn handle_agent_step(&mut self, agent_id: AgentId, step: AgentStep) -> Result<()> {
        match step {
            AgentStep::TextDelta(text) => {
                self.chat.transcript.stream_delta(BlockType::Text, &text);
            },
            AgentStep::CompactionDelta(text) => {
                self.chat
                    .transcript
                    .stream_delta(BlockType::Compaction, &text);
            },
            AgentStep::ThinkingDelta(text) => {
                self.chat
                    .transcript
                    .stream_delta(BlockType::Thinking, &text);
            },
            AgentStep::ToolRequest(tool_calls) => {
                // Set agent_id on each tool call before enqueuing
                let tool_calls: Vec<_> = tool_calls
                    .into_iter()
                    .map(|tc| tc.with_agent_id(agent_id))
                    .collect();
                self.tool_executor.enqueue(tool_calls);
            },
            AgentStep::Retrying { attempt, error } => {
                self.alert = Some(format!(
                    "Request failed (attempt {}): {}. Retrying...",
                    attempt, error
                ));
                tracing::warn!("Retrying request: attempt {}, error: {}", attempt, error);
            },
            AgentStep::Finished { usage } => {
                self.input_mode = InputMode::Normal;

                // Handle compaction completion
                // TODO something more robust than checking active block type
                if self
                    .chat
                    .transcript
                    .is_streaming_block_type(BlockType::Compaction)
                {
                    self.chat.transcript.finish_turn();
                    if let Err(e) = self.chat.transcript.save() {
                        tracing::error!("Failed to save transcript before compaction: {}", e);
                    }
                    match self.chat.transcript.rotate() {
                        Ok(new_transcript) => {
                            tracing::info!(
                                "Compaction complete, rotating to {:?}",
                                new_transcript.path()
                            );
                            self.chat
                                .reset_transcript(new_transcript, &mut self.terminal);
                            self.draw();
                        },
                        Err(e) => {
                            tracing::error!("Failed to rotate transcript: {}", e);
                        },
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
            },
            AgentStep::Error(msg) => {
                self.chat.transcript.mark_active_block(Status::Error);
                if let Err(e) = self.chat.transcript.save() {
                    tracing::error!("Failed to save transcript on error: {}", e);
                }
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
            },
        }

        // Update display (throttled during streaming)
        self.render_and_draw_throttled();

        Ok(())
    }

    /// Execute a tool decision (approve/deny) for the currently active tool
    async fn decide_pending_tool(&mut self, decision: ToolDecision) {
        tracing::debug!("decide_pending_tool: decision={:?}", decision);

        // Pop the front of the queue (the currently displayed tool)
        let (tool_call, responder) = match self.pending_approvals.pop_front() {
            Some(t) => t,
            None => {
                tracing::warn!("No tool awaiting approval");
                return;
            },
        };

        // Update block status based on decision
        if let Some(block) = self.chat.transcript.find_tool_block_mut(&tool_call.call_id) {
            block.set_status(match decision {
                ToolDecision::Approve => Status::Running,
                ToolDecision::Deny => Status::Denied,
                _ => Status::Pending,
            });
        }

        // Send decision to executor
        tracing::debug!("decide_pending_tool: sending decision for {}", tool_call.call_id);
        if responder.send(decision).is_err() {
            tracing::warn!("Failed to send decision (receiver dropped)");
        }

        // If there are more tools queued, activate the next one
        if let Some((next_call, _)) = self.pending_approvals.front() {
            tracing::debug!("decide_pending_tool: activating next tool {}", next_call.call_id);
            // Create block for the next tool
            let tool = self.tool_executor.tools().get(&next_call.name);
            self.chat.start_block(
                tool.create_block(&next_call.call_id, next_call.params.clone(), next_call.background),
                &mut self.terminal,
            );
            self.draw(); // flush block before showing approval UI
            self.input_mode = InputMode::ToolApproval;
        } else {
            // No more pending - switch to streaming mode
            self.input_mode = InputMode::Streaming;
        }

        self.chat.render(&mut self.terminal);
        self.draw();
    }

    /// Handle events from the tool executor
    async fn handle_tool_event(&mut self, event: ToolEvent) -> Result<()> {
        tracing::debug!(
            "handle_tool_event: {:?}",
            match &event {
                ToolEvent::AwaitingApproval { name, .. } => format!("AwaitingApproval({})", name),
                ToolEvent::Delegate { effect, .. } => format!("Delegate({:?})", effect),
                ToolEvent::Delta { .. } => "Delta".to_string(),
                ToolEvent::Completed { content, .. } =>
                    format!("Completed(content_len={})", content.len()),
                ToolEvent::Error { content, .. } => format!("Error(content_len={})", content.len()),
                ToolEvent::BackgroundStarted { name, call_id, .. } =>
                    format!("BackgroundStarted({}, {})", name, call_id),
                ToolEvent::BackgroundCompleted { name, call_id, .. } =>
                    format!("BackgroundCompleted({}, {})", name, call_id),
            }
        );

        match event {
            ToolEvent::AwaitingApproval {
                agent_id,
                call_id,
                name,
                params,
                background,
                responder,
            } => {
                let is_primary = self.agents.primary_id() == Some(agent_id);

                if is_primary {
                    // Create ToolCall and add to queue
                    let tool_call = ToolCall {
                        agent_id,
                        call_id: call_id.clone(),
                        name: name.clone(),
                        params: params.clone(),
                        decision: ToolDecision::Pending,
                        background,
                    };
                    let is_first = self.pending_approvals.is_empty();
                    self.pending_approvals.push_back((tool_call, responder));

                    // Only create block and show UI for the first tool in queue
                    if is_first {
                        self.draw(); // flush any pending text
                        let tool = self.tool_executor.tools().get(&name);
                        self.chat.start_block(
                            tool.create_block(&call_id, params.clone(), background),
                            &mut self.terminal,
                        );
                        self.draw();

                        // Check filters for auto-approve/deny
                        match self.tool_filters.evaluate(&name, &params) {
                            Some(decision) => {
                                self.decide_pending_tool(decision).await;
                            },
                            None => {
                                // Wait for user approval
                                if let Err(e) = self.chat.transcript.save() {
                                    tracing::error!(
                                        "Failed to save transcript before tool approval: {}",
                                        e
                                    );
                                }
                                self.input_mode = InputMode::ToolApproval;
                            },
                        }
                    }
                    // If not first, it's queued - no UI needed yet
                } else {
                    // Background agents: auto-approve without UI
                    tracing::debug!(
                        "Auto-approving tool {} for background agent {}",
                        name,
                        agent_id
                    );
                    let _ = responder.send(ToolDecision::Approve);
                }
            },

            ToolEvent::Delegate {
                agent_id,
                effect,
                responder,
                ..
            } => {
                let result = match self.apply_effect(agent_id, effect).await {
                    Ok(output) => Ok(output),
                    Err(e) => Err(e.to_string()),
                };
                // Send result back to executor (ignore if receiver dropped)
                let _ = responder.send(result);
            },

            ToolEvent::Delta {
                agent_id,
                call_id,
                content,
            } => {
                // Only stream output to transcript for primary agent
                let is_primary = self.agents.primary_id() == Some(agent_id);
                if is_primary {
                    if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                        block.append_text(&content);
                        // Re-render to show the delta (throttled)
                        self.render_and_draw_throttled();
                    } else {
                        tracing::warn!("No block found for call_id: {}", call_id);
                    }
                }
            },

            ToolEvent::Completed {
                agent_id,
                call_id,
                content,
            } => {
                let is_primary = self.agents.primary_id() == Some(agent_id);

                if is_primary {
                    // Set the output on the block and mark it complete (by call_id, not active block)
                    if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                        block.append_text(&content);
                        block.set_status(Status::Complete);
                    }
                }

                // Tell agent about the result - route to the correct agent by ID
                if let Some(agent_mutex) = self.agents.get(agent_id) {
                    agent_mutex
                        .lock()
                        .await
                        .submit_tool_result(&call_id, content);
                }

                if is_primary {
                    // Tool is done - only switch to streaming if no more approvals pending
                    if self.pending_approvals.is_empty() {
                        self.input_mode = InputMode::Streaming;
                    }

                    // Render update
                    self.chat.render(&mut self.terminal);
                    self.draw();
                }
            },

            ToolEvent::Error {
                agent_id,
                call_id,
                content,
            } => {
                let is_primary = self.agents.primary_id() == Some(agent_id);

                if is_primary {
                    // Set the output on the block before marking error
                    if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                        block.append_text(&content);
                    }

                    // Update transcript status for primary agent
                    self.chat.transcript.mark_active_block(Status::Error);
                }

                // Tell agent about the error - route to the correct agent by ID
                if let Some(agent_mutex) = self.agents.get(agent_id) {
                    agent_mutex
                        .lock()
                        .await
                        .submit_tool_result(&call_id, content);
                }

                if is_primary {
                    // Tool is done - only switch to streaming if no more approvals pending
                    if self.pending_approvals.is_empty() {
                        self.input_mode = InputMode::Streaming;
                    }

                    // Render update
                    self.chat.render(&mut self.terminal);
                    self.draw();
                }
            },

            ToolEvent::BackgroundStarted {
                agent_id,
                call_id,
                name,
            } => {
                // Background task started - send placeholder result to agent
                let placeholder = format!("Running in background (task_id: {})", call_id);
                if let Some(agent_mutex) = self.agents.get(agent_id) {
                    agent_mutex
                        .lock()
                        .await
                        .submit_tool_result(&call_id, placeholder);
                }
                tracing::info!("Background task started: {} ({})", name, call_id);
            },

            ToolEvent::BackgroundCompleted {
                agent_id,
                call_id,
                name,
            } => {
                // Background task completed - status available via list_background_tasks
                tracing::info!(
                    "Background task completed: {} ({}) for agent {}",
                    name,
                    call_id,
                    agent_id
                );
                // Redraw to update background task indicator
                self.draw();
            },
        }
        Ok(())
    }

    /// Apply a tool effect. Returns Ok(Some(output)) to set pipeline output.
    async fn apply_effect(&mut self, _agent_id: AgentId, effect: Effect) -> Result<Option<String>> {
        tracing::debug!("Applying effect: {:?}", effect);
        match effect {
            Effect::IdeReloadBuffer { path } => {
                if let Some(ide) = &self.ide {
                    ide.reload_buffer(&path.to_string_lossy()).await?;
                }
                Ok(None)
            },
            Effect::IdeOpen { path, line, column } => {
                if let Some(ide) = &self.ide {
                    ide.navigate_to(&path.to_string_lossy(), line, column)
                        .await?;
                }
                Ok(None)
            },
            Effect::IdeShowPreview { preview } => {
                if let Some(ide) = &self.ide {
                    ide.show_preview(&preview).await?;
                }
                Ok(None)
            },
            Effect::IdeShowDiffPreview { path, edits } => {
                if let Some(ide) = &self.ide {
                    ide.show_diff_preview(&path.to_string_lossy(), &edits)
                        .await?;
                }
                Ok(None)
            },
            Effect::IdeClosePreview => {
                if let Some(ide) = &self.ide {
                    ide.close_preview().await?;
                }
                Ok(None)
            },
            Effect::IdeCheckUnsavedEdits { path } => {
                if let Some(ide) = &self.ide {
                    if ide.has_unsaved_changes(&path.to_string_lossy()).await? {
                        anyhow::bail!(
                            "File {} has unsaved changes in the IDE. Save or discard them first.",
                            path.display()
                        );
                    }
                }
                Ok(None)
            },
            Effect::ListBackgroundTasks => {
                let tasks = self.tool_executor.list_tasks();
                if tasks.is_empty() {
                    Ok(Some("No background tasks".to_string()))
                } else {
                    let output = tasks
                        .iter()
                        .map(|(call_id, name, status)| {
                            format!("{} ({}) [{:?}]", call_id, name, status)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Some(output))
                }
            },
            Effect::GetBackgroundTask { task_id } => {
                match self.tool_executor.take_result(&task_id) {
                    Some((tool_name, output, status)) => Ok(Some(format!(
                        "Task {} ({}) [{:?}]:\n{}",
                        task_id, tool_name, status, output
                    ))),
                    None => Ok(Some(format!("Task {} not found or still running", task_id))),
                }
            },
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.restore_terminal();
    }
}
