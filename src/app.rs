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

use crate::commands::Command;
use crate::config::{AgentRuntimeConfig, Config};
use crate::effect::{Effect, EffectPoll, EffectQueue, PendingEffect};
use crate::ide::{Ide, IdeEvent, Nvim};
use crate::llm::{Agent, AgentId, AgentRegistry, AgentStatus, AgentStep, RequestMode};
#[cfg(feature = "profiling")]
use crate::{profile_frame, profile_span};
use crate::prompts::{SystemPrompt, COMPACTION_PROMPT, WELCOME_MESSAGE};
use crate::tool_filter::ToolFilters;
use crate::tools::{
    init_agent_context, init_browser_context, update_agent_oauth, EffectResult,
    ToolDecision, ToolEvent, ToolExecutor, ToolRegistry,
};
use crate::transcript::{BlockType, Role, Status, TextBlock, Transcript};
use crate::ui::{Attachment, ChatView, InputBox};

const MIN_FRAME_TIME: Duration = Duration::from_millis(16);

pub const APP_NAME: &str = "Codey";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// Notification sources for mid-turn injections
#[derive(Debug, Clone)]
pub enum NotificationSource {
    /// User sent a message while agent was streaming
    User,
    /// File was modified externally
    FileWatcher,
    /// Background task completed
    BackgroundTask,
    /// IDE event (diagnostics, etc.)
    Ide,
}

impl std::fmt::Display for NotificationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotificationSource::User => write!(f, "user"),
            NotificationSource::FileWatcher => write!(f, "file_watcher"),
            NotificationSource::BackgroundTask => write!(f, "background_task"),
            NotificationSource::Ide => write!(f, "ide"),
        }
    }
}

/// A notification to be injected into tool results
#[derive(Debug, Clone)]
pub struct Notification {
    pub source: NotificationSource,
    pub message: String,
}

impl Notification {
    pub fn new(source: NotificationSource, message: impl Into<String>) -> Self {
        Self {
            source,
            message: message.into(),
        }
    }

    /// Format as XML for injection into tool results
    pub fn to_xml(&self) -> String {
        format!(
            "<notification source=\"{}\">\n{}\n</notification>",
            self.source, self.message
        )
    }
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
    /// Queue for pending effects (approvals, IDE previews, etc.)
    effects: EffectQueue,
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
            effects: EffectQueue::new(),
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

        // Initialize browser context for fetch_html
        init_browser_context(&self.config.browser);

        // Use dynamic prompt builder so mdsh commands are re-executed on each LLM call
        let system_prompt = SystemPrompt::new();
        let mut agent = Agent::with_dynamic_prompt(
            AgentRuntimeConfig::foreground(&self.config),
            Box::new(move || system_prompt.build()),
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
                // Handle pending effects (IDE previews waiting for slot, etc.)
                _ = std::future::ready(()), if self.effects.has_pollable() => {
                    if let Some(pending) = self.effects.poll_next() {
                        self.handle_pending_effect(pending).await;
                        // Ensure UI is updated after handling effects (especially for background agent approvals)
                        self.draw();
                    }
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
                self.tool_executor.running_background_count() + self.agents.running_background_count(),
                self.input_mode != InputMode::Normal,
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
        let is_primary = self.agents.primary_id() == Some(agent_id);

        match step {
            AgentStep::TextDelta(text) => {
                if !is_primary { return Ok(()); }
                self.chat.transcript.stream_delta(BlockType::Text, &text);
            },
            AgentStep::CompactionDelta(text) => {
                if !is_primary { return Ok(()); }
                self.chat
                    .transcript
                    .stream_delta(BlockType::Compaction, &text);
            },
            AgentStep::ThinkingDelta(text) => {
                if !is_primary { return Ok(()); }
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
                if is_primary {
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
                } else {
                    // Sub-agent finished - just mark as finished and log
                    // Result will be retrieved via get_agent tool when primary agent requests it
                    let label = self.agents.metadata(agent_id)
                        .map(|m| m.label.clone())
                        .unwrap_or_default();
                    self.agents.finish(agent_id);
                    tracing::info!("Sub-agent {} ({}) finished", agent_id, label);
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

    /// Handle a pending effect - execute it and either complete or re-queue.
    async fn handle_pending_effect(&mut self, mut pending: PendingEffect) {
        // Special handling for approval effects
        if let Effect::AwaitApproval { ref name, ref params, background } = pending.effect {
            if !pending.acknowledged {
                // Clone values before mutating pending
                let call_id = pending.call_id.clone();
                let agent_id = pending.agent_id;
                let name = name.clone();
                let params = params.clone();
                
                // Mark as acknowledged and requeue BEFORE showing UI
                // This is needed because acknowledge_approval may auto-decide,
                // which calls decide_pending_tool, which looks for the effect in the queue
                pending.acknowledge();
                self.effects.requeue(pending);
                
                // Now show approval UI (may auto-decide)
                self.acknowledge_approval(&call_id, agent_id, &name, params, background).await;
                return;
            }
            // Already acknowledged - stays in queue waiting for user input
            self.effects.requeue(pending);
            return;
        }
        
        // IDE preview effects need polling (resource contention)
        if matches!(pending.effect, Effect::IdeShowPreview { .. } | Effect::IdeShowDiffPreview { .. }) {
            let poll_result = self.try_execute_effect(&pending.effect).await;
            match poll_result {
                EffectPoll::Ready(result) => {
                    pending.complete(result.map_err(|e: anyhow::Error| e.to_string()));
                }
                EffectPoll::Pending => {
                    self.effects.requeue(pending);
                }
            }
            return;
        }
        
        // All other effects execute immediately
        let PendingEffect { agent_id, effect, responder, .. } = pending;
        let result = self.apply_effect(agent_id, effect).await;
        let _ = responder.send(result.map_err(|e| e.to_string()));
    }
    
    /// Show approval UI for an effect (create block, check filters, set mode)
    async fn acknowledge_approval(
        &mut self,
        call_id: &str,
        agent_id: AgentId,
        name: &str,
        params: serde_json::Value,
        background: bool,
    ) {
        let is_primary = self.agents.primary_id() == Some(agent_id);
        
        // Get agent label for sub-agents
        let agent_label = if !is_primary {
            self.agents.metadata(agent_id).map(|m| m.label.clone())
        } else {
            None
        };
        
        self.draw(); // flush any pending text
        let tool = self.tool_executor.tools().get(name);
        let mut block = tool.create_block(call_id, params.clone(), background);
        
        // Set agent label for sub-agent tools
        if let Some(ref label) = agent_label {
            tracing::info!("Sub-agent '{}' requesting approval for {}", label, name);
            block.set_agent_label(label.clone());
        }
        
        self.chat.start_block(block, &mut self.terminal);
        self.draw();
        
        // Check filters for auto-approve/deny
        match self.tool_filters.evaluate(name, &params) {
            Some(decision) => {
                self.decide_pending_tool(decision).await;
            }
            None => {
                // Wait for user approval
                if is_primary {
                    if let Err(e) = self.chat.transcript.save() {
                        tracing::error!("Failed to save transcript before tool approval: {}", e);
                    }
                }
                self.input_mode = InputMode::ToolApproval;
                self.draw();
            }
        }
    }
    
    /// Try to execute an IDE preview effect. Returns Ready if completed, Pending if slot not available.
    /// Only handles IdeShowPreview and IdeShowDiffPreview - all other effects go through apply_effect.
    async fn try_execute_effect(&mut self, effect: &Effect) -> EffectPoll {
        match effect {
            Effect::IdeShowPreview { preview } => {
                if let Some(ide) = &self.ide {
                    match ide.try_claim_preview().await {
                        Ok(true) => {
                            match ide.show_preview(preview).await {
                                Ok(_) => EffectPoll::Ready(Ok(None)),
                                Err(e) => EffectPoll::Ready(Err(e)),
                            }
                        }
                        Ok(false) => EffectPoll::Pending,
                        Err(e) => EffectPoll::Ready(Err(e)),
                    }
                } else {
                    EffectPoll::Ready(Ok(None))
                }
            }
            
            Effect::IdeShowDiffPreview { path, edits } => {
                if let Some(ide) = &self.ide {
                    match ide.try_claim_preview().await {
                        Ok(true) => {
                            match ide.show_diff_preview(&path.to_string_lossy(), edits).await {
                                Ok(_) => EffectPoll::Ready(Ok(None)),
                                Err(e) => EffectPoll::Ready(Err(e)),
                            }
                        }
                        Ok(false) => EffectPoll::Pending,
                        Err(e) => EffectPoll::Ready(Err(e)),
                    }
                } else {
                    EffectPoll::Ready(Ok(None))
                }
            }
            
            // All other effects should go through apply_effect, not here
            _ => EffectPoll::Ready(Err(anyhow::anyhow!(
                "Effect {:?} should not be polled - use apply_effect", effect
            ))),
        }
    }
    
    /// Get output from an agent by label (for GetAgent effect)
    #[cfg(feature = "cli")]
    async fn get_agent_output(&mut self, label: &str) -> String {
        // Find agent by label
        let agent_id = match self.agents.find_by_label(label) {
            Some(id) => id,
            None => return format!("Agent '{}' not found", label),
        };

        // Check status
        let status = self.agents.metadata(agent_id)
            .map(|m| m.status.clone());

        match status {
            Some(status) => {
                let status_str = format!("{:?}", status);
                if status == AgentStatus::Finished {
                    // Get result and remove agent (consume on retrieval)
                    let result = if let Some(agent_mutex) = self.agents.get(agent_id) {
                        let agent = agent_mutex.lock().await;
                        agent.last_message().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    self.agents.remove(agent_id);
                    format!("[{}] [{}]:\n{}", label, status_str, result)
                } else {
                    // Still running - just return status
                    format!("[{}] [{}]", label, status_str)
                }
            }
            None => format!("Agent '{}' status unknown", label),
        }
    }

    /// Execute a tool decision (approve/deny) for the currently active tool
    async fn decide_pending_tool(&mut self, decision: ToolDecision) {
        tracing::debug!("decide_pending_tool: decision={:?}", decision);

        // Take the currently active approval
        let pending = match self.effects.take_active_approval() {
            Some(p) => p,
            None => {
                tracing::warn!("No tool awaiting approval");
                return;
            },
        };

        // Update block status based on decision
        if let Some(block) = self.chat.transcript.find_tool_block_mut(&pending.call_id) {
            block.set_status(match decision {
                ToolDecision::Approve => Status::Running,
                ToolDecision::Deny => Status::Denied,
                _ => Status::Pending,
            });
        }

        // Convert decision to EffectResult and send to executor
        let result: EffectResult = match decision {
            ToolDecision::Approve => Ok(None),
            ToolDecision::Deny => Err("Denied by user".to_string()),
            _ => Err("Unexpected decision".to_string()),
        };
        tracing::debug!("decide_pending_tool: sending decision for {}", pending.call_id);
        pending.complete(result);

        // If no more acknowledged approvals, switch mode
        // (next unacknowledged approval will be handled by polling)
        if !self.effects.has_active_approval() {
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
            ToolEvent::Delegate {
                agent_id,
                call_id,
                effect,
                responder,
                ..
            } => {
                // Queue effect for polling
                self.effects.push(PendingEffect::new(call_id, agent_id, effect, responder));
            },

            ToolEvent::Delta {
                call_id,
                content,
                ..
            } => {
                // Stream output to block (works for both primary and sub-agent tools)
                if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                    block.append_text(&content);
                    // Re-render to show the delta (throttled)
                    self.render_and_draw_throttled();
                } else {
                    tracing::warn!("No block found for call_id: {}", call_id);
                }
            },

            ToolEvent::Completed {
                agent_id,
                call_id,
                content,
            } => {
                let is_primary = self.agents.primary_id() == Some(agent_id);

                // Update block status (works for both primary and sub-agent tools)
                if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                    block.append_text(&content);
                    block.set_status(Status::Complete);
                }

                // Tell agent about the result - route to the correct agent by ID
                if let Some(agent_mutex) = self.agents.get(agent_id) {
                    agent_mutex
                        .lock()
                        .await
                        .submit_tool_result(&call_id, content);
                }

                // Tool is done - only switch to streaming if no more approvals pending
                if !self.effects.has_pending_approvals() {
                    self.input_mode = InputMode::Streaming;
                }

                // Render update
                self.chat.render(&mut self.terminal);
                self.draw();
            },

            ToolEvent::Error {
                agent_id,
                call_id,
                content,
            } => {
                // Update block status (works for both primary and sub-agent tools)
                if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                    block.append_text(&content);
                    block.set_status(Status::Error);
                }

                // Tell agent about the error - route to the correct agent by ID
                if let Some(agent_mutex) = self.agents.get(agent_id) {
                    agent_mutex
                        .lock()
                        .await
                        .submit_tool_result(&call_id, content);
                }

                // Tool is done - only switch to streaming if no more approvals pending
                if !self.effects.has_pending_approvals() {
                    self.input_mode = InputMode::Streaming;
                }

                // Render update
                self.chat.render(&mut self.terminal);
                self.draw();
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
            // AwaitApproval is handled via the EffectQueue, not here
            Effect::AwaitApproval { .. } => {
                unreachable!("AwaitApproval should be handled via EffectQueue, not apply_effect")
            },
            // Preview effects are polled via try_execute_effect, not here
            Effect::IdeShowPreview { .. } | Effect::IdeShowDiffPreview { .. } => {
                unreachable!("IDE preview effects should be polled via try_execute_effect, not apply_effect")
            },
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
            Effect::SpawnAgent {
                agent,
                label,
            } => {
                // Register the agent - it will be polled through agents.next()
                let parent_id = _agent_id;
                let agent_id = self.agents.register_spawned(agent, label.clone(), parent_id);
                tracing::info!("Spawned sub-agent {} with label '{}'", agent_id, label);
                // Return the agent ID so the handler knows what was spawned
                Ok(Some(format!("agent:{}", agent_id)))
            },
            Effect::ListAgents => {
                let spawned = self.agents.list_spawned();
                if spawned.is_empty() {
                    Ok(Some("No spawned agents".to_string()))
                } else {
                    let output = spawned
                        .iter()
                        .enumerate()
                        .map(|(i, (_, meta))| {
                            let elapsed = meta.created_at.elapsed().as_secs();
                            format!(
                                "{}. {} [{:?}] {}s",
                                i + 1, meta.label, meta.status, elapsed
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Some(output))
                }
            },
            Effect::GetAgent { label } => {
                // Find agent by label
                let agent_id = match self.agents.find_by_label(&label) {
                    Some(id) => id,
                    None => return Ok(Some(format!("Agent '{}' not found", label))),
                };

                // Check status
                let status = self.agents.metadata(agent_id)
                    .map(|m| m.status.clone());

                match status {
                    Some(status) => {
                        let status_str = format!("{:?}", status);
                        if status == AgentStatus::Finished {
                            // Get result and remove agent (consume on retrieval)
                            let result = if let Some(agent_mutex) = self.agents.get(agent_id) {
                                let agent = agent_mutex.lock().await;
                                agent.last_message().unwrap_or_default()
                            } else {
                                String::new()
                            };
                            self.agents.remove(agent_id);
                            Ok(Some(format!(
                                "[{}] [{}]:\n{}",
                                label, status_str, result
                            )))
                        } else {
                            // Still running - just return status
                            Ok(Some(format!(
                                "[{}] [{}]",
                                label, status_str
                            )))
                        }
                    }
                    None => Ok(Some(format!("Agent '{}' not found", label))),
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
