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
use crate::effect::{Effect, EffectQueue, PendingEffect};
use crate::llm::{Agent, AgentId, AgentRegistry, AgentStep, RequestMode};
#[cfg(feature = "profiling")]
use crate::{profile_frame, profile_span};
use crate::notifications::{Notification, NotificationQueue};
use crate::prompts::{SystemPrompt, welcome_message};
use crate::tools::{init_agent_context, ToolEvent, ToolExecutor, ToolRegistry};
use crate::transcript::{Block, BlockType, NotificationBlock, Role, Status, TextBlock, Transcript};
use crate::ui::{ActivityReadout, Attachment, ChatView, InputBox};

const MIN_FRAME_TIME: Duration = Duration::from_millis(16);
/// How often the activity readout redraws while a turn runs: often enough for its
/// token count to move smoothly.
const ACTIVITY_FRAME: Duration = Duration::from_millis(50);

pub const APP_NAME: &str = "Codey";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Result of handling an action
#[allow(dead_code)]
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
    /// Last render time for frame rate limiting
    last_render: Instant,
    /// Redraws the activity readout (clock, smoothed token count) while a turn runs.
    activity_tick: tokio::time::Interval,
    /// The running turn's readout, shown in the input box's title
    activity: ActivityReadout,
    /// Alert message to display (cleared on next user input)
    alert: Option<String>,
    /// Terminal event stream
    events: EventStream,
    /// Current input mode
    input_mode: InputMode,
    /// Agent registry for managing multiple agents
    agents: AgentRegistry,
    /// Tool executor for running tools
    tool_executor: ToolExecutor,
    /// Queue for pending effects (sub-agent spawns and listings)
    effects: EffectQueue,
    /// Notifications to inject into next tool result
    notifications: NotificationQueue,
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

        let agent_name = config.agent.name().to_string();

        Ok(Self {
            config,
            terminal,
            chat: ChatView::new(transcript, terminal_size.0, chat_height, agent_name),
            input: InputBox::new(),
            should_quit: false,
            continue_session,
            last_render: Instant::now(),
            activity_tick: {
                let mut tick = tokio::time::interval(ACTIVITY_FRAME);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                tick
            },
            activity: ActivityReadout::default(),
            alert: None,
            events: EventStream::new(),
            input_mode: InputMode::Normal,
            agents: AgentRegistry::new(),
            tool_executor: ToolExecutor::new(ToolRegistry::new()),
            effects: EffectQueue::new(),
            notifications: NotificationQueue::new(),
        })
    }

    /// Run the main event loop - purely event-driven rendering
    pub async fn run(&mut self) -> Result<()> {
        // Initialize agent context for sub-agent spawning
        init_agent_context(AgentRuntimeConfig::background(&self.config));

        // Use dynamic prompt builder so mdsh commands are re-executed on each LLM call
        let system_prompt = SystemPrompt::with_config(&self.config);
        let agent_name = system_prompt.agent_name().to_string();
        let mut agent = Agent::with_dynamic_prompt(
            AgentRuntimeConfig::foreground(&self.config),
            Box::new(move || system_prompt.build()),
            self.tool_executor.tools().clone(),
        );

        if self.continue_session {
            agent.restore_from_transcript(&self.chat.transcript);
        } else {
            self.chat
                .add_turn(Role::Assistant, TextBlock::pending(&welcome_message(&agent_name)));
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
                // Handle agent steps (streaming responses, tool requests)
                Some((agent_id, agent_step)) = self.agents.next() => {
                    self.handle_agent_step(agent_id, agent_step).await?;
                }
                // Handle tool executor events (tool output, completion)
                Some(tool_event) = self.tool_executor.next() => {
                    self.handle_tool_event(tool_event).await?;
                }
                // Advance the activity readout while a turn runs
                _ = self.activity_tick.tick(), if self.input_mode != InputMode::Normal => {
                    self.draw();
                }
                // Handle notifications when in normal input mode
                _ = std::future::ready(()), if self.input_mode == InputMode::Normal && !self.notifications.is_empty() => {
                    let notifications = self.notifications.drain_all();
                    self.handle_notifications(notifications).await?;
                }
                // Handle pending effects (IDE previews waiting for slot, etc.)
                _ = std::future::ready(()), if self.effects.has_pollable() => {
                    if let Some(pending) = self.effects.poll_next() {
                        self.handle_pending_effect(pending).await;
                        // Ensure UI is updated after handling effects
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

    /// Cancel the current activity, with layered behavior:
    /// 1. If a foreground tool is running → cancel just that tool (error sent to LLM)
    /// 2. Otherwise → cancel the entire turn (agent stream + all tools)
    async fn cancel(&mut self) -> Result<()> {
        // Layer 1: If a foreground tool is running, just cancel it.
        // The error result flows back to the LLM, which continues its turn.
        if self.tool_executor.has_running_foreground() {
            let events = self.tool_executor.cancel_foreground();
            for event in events {
                self.handle_tool_event(event).await?;
            }
            return Ok(());
        }

        // Layer 2: No foreground tool running — cancel the entire turn.
        if let Some(agent_mutex) = self.agents.primary() {
            agent_mutex.lock().await.cancel();
        }
        self.tool_executor.cancel();
        self.effects.clear();
        self.chat.finish_turn(&mut self.terminal);
        if let Err(e) = self.chat.transcript.save() {
            tracing::error!("Failed to save transcript on cancel: {}", e);
        }
        self.input_mode = InputMode::Normal;
        Ok(())
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
        let (usage, activity) = self
            .agents
            .primary()
            .and_then(|m| m.try_lock().ok())
            .map_or((Default::default(), None), |a| (a.total_usage(), a.activity()));
        let activity = activity.map(|a| self.activity.text(&a));
        let chat_widget = self.chat.widget();
        let input_widget = self.input.widget(
            activity,
            usage.context_tokens,
            usage.tokens_per_second,
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

    /// Queue a user message or command for processing.
    /// Stages a block for rendering and queues a notification for processing.
    fn queue_message(&mut self, content: String) {
        if let Some(command) = Command::parse(&content) {
            let name = command.name().to_string();
            // Stage block for visual feedback
            let block = TextBlock::pending(format!("/{}", name));
            let block_id = self.chat.transcript.stage.push(Box::new(block));
            self.notifications
                .push(Notification::Command { name, block_id });
        } else {
            // Stage block for visual feedback
            let block = TextBlock::pending(&content);
            let block_id = self.chat.transcript.stage.push(Box::new(block));
            self.notifications
                .push(Notification::Message { content, block_id });
        }

        self.chat.render(&mut self.terminal);
        self.draw();
    }

    fn set_input_mode(&mut self, mode: InputMode) {
        self.input_mode = mode;
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

    /// Handle notifications from the queue.
    /// Batches messages together into a single request, executes commands individually.
    async fn handle_notifications(&mut self, notifications: Vec<Notification>) -> Result<()> {
        // Separate by type: messages get batched, commands execute individually
        let mut messages: Vec<(String, usize)> = Vec::new();  // (content, block_id)
        let mut commands: Vec<(String, usize)> = Vec::new();  // (name, block_id)
        let mut background_tools: Vec<(String, String, usize)> = Vec::new();  // (label, result, block_id)
        let mut background_agents: Vec<(String, String, usize)> = Vec::new();  // (label, result, block_id)

        for notification in notifications {
            match notification {
                Notification::Message { content, block_id } => {
                    messages.push((content, block_id));
                }
                Notification::Command { name, block_id } => {
                    commands.push((name, block_id));
                }
                Notification::BackgroundTool { label, result, block_id } => {
                    background_tools.push((label, result, block_id));
                }
                Notification::BackgroundAgent { label, result, block_id } => {
                    background_agents.push((label, result, block_id));
                }
            }
        }

        // Process commands first (they don't send to agent, execute locally)
        for (name, block_id) in commands {
            // Promote block from stage to transcript
            if let Some(mut block) = self.chat.transcript.stage.remove(block_id) {
                block.set_status(Status::Complete);
                let turn_id = self.chat.transcript.add_empty(Role::User);
                if let Some(turn) = self.chat.transcript.get_mut(turn_id) {
                    turn.add_block(block);
                }
            }

            if let Some(command) = Command::get(&name) {
                match command.execute(self) {
                    Ok(None) => {
                        self.chat.render(&mut self.terminal);
                        self.draw();
                    }
                    Ok(Some(output)) => {
                        let idx = self.chat.transcript.add_empty(Role::Assistant);
                        if let Some(turn) = self.chat.transcript.get_mut(idx) {
                            turn.start_block(Box::new(TextBlock::complete(&output)));
                        }
                        self.chat.render(&mut self.terminal);
                        self.draw();
                    }
                    Err(e) => {
                        tracing::error!("Command execution error: {}", e);
                        self.alert = Some(format!("Command error: {}", e));
                    }
                }
            }
        }

        // Combine messages and background tasks into a single turn and request
        let mut combined_content = String::new();
        let mut blocks_to_promote: Vec<Box<dyn Block>> = Vec::new();

        for (content, block_id) in &messages {
            // Collect block for promotion
            if let Some(mut block) = self.chat.transcript.stage.remove(*block_id) {
                block.set_status(Status::Complete);
                blocks_to_promote.push(block);
            }
            if !combined_content.is_empty() {
                combined_content.push_str("\n\n");
            }
            combined_content.push_str(content);
        }

        for (label, result, block_id) in &background_tools {
            // Collect block for promotion
            if let Some(mut block) = self.chat.transcript.stage.remove(*block_id) {
                block.set_status(Status::Complete);
                blocks_to_promote.push(block);
            }
            let content = format!("Background tool '{}' completed:\n{}", label, result);
            if !combined_content.is_empty() {
                combined_content.push_str("\n\n");
            }
            combined_content.push_str(&content);
        }

        for (label, result, block_id) in &background_agents {
            // Collect block for promotion
            if let Some(mut block) = self.chat.transcript.stage.remove(*block_id) {
                block.set_status(Status::Complete);
                blocks_to_promote.push(block);
            }
            let content = format!("Background agent '{}' completed:\n{}", label, result);
            if !combined_content.is_empty() {
                combined_content.push_str("\n\n");
            }
            combined_content.push_str(&content);
        }

        // Create single user turn with all blocks
        if !blocks_to_promote.is_empty() {
            let turn_id = self.chat.transcript.add_empty(Role::User);
            if let Some(turn) = self.chat.transcript.get_mut(turn_id) {
                for block in blocks_to_promote {
                    turn.add_block(block);
                }
            }
        }

        // Send combined message to agent (if any)
        if !combined_content.is_empty() {
            self.chat.render(&mut self.terminal);
            self.draw();

            if let Some(agent_mutex) = self.agents.primary() {
                agent_mutex
                    .lock()
                    .await
                    .send_request(&combined_content, RequestMode::Normal);
            }
            self.chat.begin_turn(Role::Assistant, &mut self.terminal);
            self.set_input_mode(InputMode::Streaming);
        }

        self.chat.render(&mut self.terminal);
        self.draw();

        Ok(())
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
                ToolEvent::Started { name, call_id, .. } =>
                    format!("Started({}, {})", name, call_id),
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
                self.effects
                    .push(PendingEffect::new(call_id, agent_id, effect, responder));
            },

            ToolEvent::Delta {
                call_id, content, ..
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
                let _is_primary = self.agents.primary_id() == Some(agent_id);

                // Update block status (works for both primary and sub-agent tools)
                if let Some(block) = self.chat.transcript.find_tool_block_mut(&call_id) {
                    block.append_text(&content);
                    block.set_status(Status::Complete);
                }

                // Drain injectable notifications and append to content
                let injectable = self.notifications.drain_injectable();
                let content = if !injectable.is_empty() {
                    // Finish current assistant turn, add user turn, start new assistant turn
                    self.chat.transcript.finish_turn();
                    
                    // Add user turn with notification blocks
                    let turn_id = self.chat.transcript.add_empty(Role::User);
                    for notification in &injectable {
                        let block_id = notification.block_id();
                        if let Some(mut block) = self.chat.transcript.stage.remove(block_id) {
                            block.set_status(Status::Complete);
                            if let Some(turn) = self.chat.transcript.get_mut(turn_id) {
                                turn.add_block(block);
                            }
                        }
                    }
                    
                    // Start new assistant turn for continuation
                    self.chat.begin_turn(Role::Assistant, &mut self.terminal);
                    
                    // Generate XML for injection
                    let xml = injectable
                        .iter()
                        .filter_map(|n| n.to_xml())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    format!("{}\n\n{}", content, xml)
                } else {
                    content
                };

                // Tell agent about the result - route to the correct agent by ID
                if let Some(agent_mutex) = self.agents.get(agent_id) {
                    agent_mutex
                        .lock()
                        .await
                        .submit_tool_result(&call_id, content);
                }

                self.set_input_mode(InputMode::Streaming);

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

                self.set_input_mode(InputMode::Streaming);

                // Render update
                self.chat.render(&mut self.terminal);
                self.draw();
            },

            ToolEvent::Started {
                agent_id,
                call_id,
                name,
                params,
                background,
            } => {
                // The tool's block joins the transcript as it starts running.
                let tool = self.tool_executor.tools().get(&name);
                let mut block = tool.create_block(&call_id, params, background);
                if self.agents.primary_id() != Some(agent_id) {
                    if let Some(label) = self.agents.metadata(agent_id).map(|m| m.label.clone()) {
                        block.set_agent_label(label);
                    }
                }
                block.set_status(Status::Running);
                // The primary agent's turn may have already finished while a
                // sub-agent's tool starts.
                if !self.chat.transcript.has_active_turn() {
                    self.chat.transcript.begin_turn(Role::Assistant);
                }
                self.chat.transcript.start_block(block);

                if background {
                    // The agent goes on with a placeholder result.
                    let placeholder = format!("Running in background (task_id: {})", call_id);
                    if let Some(agent_mutex) = self.agents.get(agent_id) {
                        agent_mutex.lock().await.submit_tool_result(&call_id, placeholder);
                    }
                    tracing::info!("Background task started: {} ({})", name, call_id);
                }

                self.chat.render(&mut self.terminal);
                self.draw();
            },

            ToolEvent::BackgroundCompleted {
                agent_id,
                call_id,
                name,
            } => {
                tracing::info!(
                    "Background task completed: {} ({}) for agent {}",
                    name,
                    call_id,
                    agent_id
                );
                
                // Take the result and queue a notification
                if let Some((task_name, output, status)) = self.tool_executor.take_result(&call_id) {
                    let result = if status == Status::Error {
                        format!("Error: {}", output)
                    } else {
                        output
                    };
                    
                    // Stage a block for the notification
                    let block = NotificationBlock::new("background_tool", format!("[{}] completed", task_name));
                    let block_id = self.chat.transcript.stage.push(Box::new(block));
                    
                    // Queue the notification
                    self.notifications.push(Notification::BackgroundTool {
                        label: call_id.clone(),
                        result,
                        block_id,
                    });
                }
                
                // Redraw to update background task indicator
                self.chat.render(&mut self.terminal);
                self.draw();
            },
        }
        Ok(())
    }

    /// Handle a single agent step during streaming
    async fn handle_agent_step(&mut self, agent_id: AgentId, step: AgentStep) -> Result<()> {
        let is_primary = self.agents.primary_id() == Some(agent_id);

        match step {
            AgentStep::TextDelta(text) => {
                if !is_primary {
                    return Ok(());
                }
                self.chat.transcript.stream_delta(BlockType::Text, &text);
            },
            AgentStep::ThinkingDelta(text) => {
                if !is_primary {
                    return Ok(());
                }
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
                let delay_secs = 2u64.pow(attempt);
                self.alert = Some(format!(
                    "Request failed (attempt {}): {}. Retrying in {}s...",
                    attempt, error, delay_secs
                ));
                tracing::warn!("Retrying request: attempt {}, error: {}, backoff: {}s", attempt, error, delay_secs);
            },
            AgentStep::Finished { usage: _ } => {
                if is_primary {
                    self.set_input_mode(InputMode::Normal);

                    // Context management / compaction is owned by the claude binary.
                    self.chat.transcript.finish_turn();
                    if let Err(e) = self.chat.transcript.save() {
                        tracing::error!("Failed to save transcript: {}", e);
                    }
                } else {
                    // Sub-agent finished - queue notification with result
                    let label = self
                        .agents
                        .metadata(agent_id)
                        .map(|m| m.label.clone())
                        .unwrap_or_default();
                    
                    // Get the result before marking as finished
                    let result = if let Some(agent_mutex) = self.agents.get(agent_id) {
                        let agent = agent_mutex.lock().await;
                        agent.last_message().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    
                    self.agents.finish(agent_id);
                    tracing::info!("Sub-agent {} ({}) finished", agent_id, label);
                    
                    // Stage a block and queue notification
                    let block = NotificationBlock::new("background_agent", format!("[{}] completed", label));
                    let block_id = self.chat.transcript.stage.push(Box::new(block));
                    self.notifications.push(Notification::BackgroundAgent {
                        label: label.clone(),
                        result,
                        block_id,
                    });
                    
                    // Render to show pending notification
                    self.chat.render(&mut self.terminal);
                    self.draw();
                }
            },
            AgentStep::Error(msg) => {
                if is_primary {
                    // The turn is over: end it, so the next message can begin another.
                    self.chat.transcript.fail_turn();
                    if let Err(e) = self.chat.transcript.save() {
                        tracing::error!("Failed to save transcript on error: {}", e);
                    }
                    self.set_input_mode(InputMode::Normal);
                } else {
                    // A sub-agent broke: the primary agent hears about it as it would
                    // hear its result, and the primary's own turn is left alone.
                    let label = self
                        .agents
                        .metadata(agent_id)
                        .map(|m| m.label.clone())
                        .unwrap_or_default();
                    self.agents.set_error(agent_id, msg.clone());
                    tracing::warn!("Sub-agent {} ({}) failed: {}", agent_id, label, msg);
                    let block = NotificationBlock::new("background_agent", format!("[{}] failed", label));
                    let block_id = self.chat.transcript.stage.push(Box::new(block));
                    self.notifications.push(Notification::BackgroundAgent {
                        label: label.clone(),
                        result: format!("The agent failed: {}", msg),
                        block_id,
                    });
                    self.chat.render(&mut self.terminal);
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
            },
        }

        // Update display (throttled during streaming)
        self.render_and_draw_throttled();

        Ok(())
    }

    /// Handle a pending effect: apply it and send the result back to its pipeline.
    async fn handle_pending_effect(&mut self, pending: PendingEffect) {
        let PendingEffect {
            agent_id,
            effect,
            responder,
            ..
        } = pending;
        let result = self.apply_effect(agent_id, effect).await;
        let _ = responder.send(result.map_err(|e| e.to_string()));
    }

    /// Apply a tool effect. Returns Ok(Some(output)) to set pipeline output.
    async fn apply_effect(&mut self, _agent_id: AgentId, effect: Effect) -> Result<Option<String>> {
        tracing::debug!("Applying effect: {:?}", effect);
        match effect {
            Effect::SpawnAgent { agent, label } => {
                // Register the agent - it will be polled through agents.next()
                let parent_id = _agent_id;
                let agent_id = self
                    .agents
                    .register_spawned(agent, label.clone(), parent_id);
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
                            format!("{}. {} [{:?}] {}s", i + 1, meta.label, meta.status, elapsed)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(Some(output))
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
