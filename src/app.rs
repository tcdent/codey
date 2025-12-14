//! Main application state and event loop

use crate::auth::{AuthStorage, Credentials, OAuthClient};
use crate::config::{AuthMethod, Config};
use crate::llm::{Agent, AgentEvent, AnthropicClient, Usage};
use crate::tools::ToolRegistry;
use crate::ui::{
    ChatView, ConnectionStatus, DisplayContent, DisplayMessage, InputBox, InputMode,
    MarkdownRenderer, PermissionDialog, PermissionHandler, PermissionRequest, PermissionResponse,
    RiskLevel, StatusBar,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};
use std::io::{self, Stdout};
use tokio::sync::{mpsc, oneshot};

const APP_NAME: &str = "Codepal";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const SYSTEM_PROMPT: &str = r#"You are Codepal, an AI coding assistant running in a terminal interface.

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
    chat: ChatView,
    input: InputBox,
    status: ConnectionStatus,
    usage: Usage,
    agent: Option<Agent>,
    credentials: Option<Credentials>,
    should_quit: bool,
    permission_dialog: Option<PermissionDialog>,
    permission_tx: Option<oneshot::Sender<PermissionResponse>>,
    markdown_renderer: MarkdownRenderer,
}

impl App {
    /// Create a new application
    pub async fn new(config: Config) -> Result<Self> {
        // Setup terminal
        enable_raw_mode().context("Failed to enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
            .context("Failed to setup terminal")?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("Failed to create terminal")?;

        let markdown_renderer = MarkdownRenderer::new().with_theme(&config.ui.theme);

        Ok(Self {
            config,
            terminal,
            chat: ChatView::new(),
            input: InputBox::new(),
            status: ConnectionStatus::Disconnected,
            usage: Usage::default(),
            agent: None,
            credentials: None,
            should_quit: false,
            permission_dialog: None,
            permission_tx: None,
            markdown_renderer,
        })
    }

    /// Run the main event loop
    pub async fn run(&mut self) -> Result<()> {
        // Authenticate
        self.authenticate().await?;

        // Create agent
        self.create_agent()?;

        // Show welcome message
        self.chat.add_message(DisplayMessage::assistant_text(
            "Welcome to Codepal! I'm your AI coding assistant. How can I help you today?",
        ));

        // Main event loop
        loop {
            // Draw UI
            self.draw()?;

            // Handle events
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key_event(key).await?;
                }
            }

            if self.should_quit {
                break;
            }
        }

        // Cleanup
        self.cleanup()?;

        Ok(())
    }

    /// Authenticate with the API
    async fn authenticate(&mut self) -> Result<()> {
        self.status = ConnectionStatus::Connecting;
        self.draw()?;

        match self.config.auth.method {
            AuthMethod::ApiKey => {
                // Try to get API key from config or environment
                let api_key = self
                    .config
                    .auth
                    .api_key
                    .clone()
                    .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok());

                match api_key {
                    Some(key) => {
                        self.credentials = Some(Credentials::ApiKey(key));
                        self.status = ConnectionStatus::Connected;
                    }
                    None => {
                        self.status = ConnectionStatus::Error(
                            "No API key found. Set ANTHROPIC_API_KEY or use OAuth.".to_string(),
                        );
                        return Err(anyhow::anyhow!("No API key configured"));
                    }
                }
            }
            AuthMethod::OAuth => {
                // Try to load existing tokens
                let storage = AuthStorage::new()?;

                if let Some(tokens) = storage.get_anthropic_oauth()? {
                    if !tokens.is_expired() {
                        self.credentials = Some(Credentials::OAuth(tokens));
                        self.status = ConnectionStatus::Connected;
                        return Ok(());
                    }

                    // Try to refresh
                    let client = OAuthClient::new();
                    match client.refresh_token(&tokens.refresh_token).await {
                        Ok(new_tokens) => {
                            storage.save_anthropic_oauth(new_tokens.clone())?;
                            self.credentials = Some(Credentials::OAuth(new_tokens));
                            self.status = ConnectionStatus::Connected;
                            return Ok(());
                        }
                        Err(_) => {
                            // Token refresh failed, need to re-authenticate
                        }
                    }
                }

                // Start device flow
                self.status = ConnectionStatus::Connecting;
                self.draw()?;

                let client = OAuthClient::new();
                let device_response = client.start_device_flow().await?;

                // Show user code
                self.chat.add_message(DisplayMessage::assistant_text(format!(
                    "Please visit: {}\n\nEnter code: {}\n\nWaiting for authorization...",
                    device_response.verification_uri, device_response.user_code
                )));
                self.draw()?;

                // Open browser
                let _ = open::that(&device_response.verification_uri);

                // Poll for token
                let interval = std::time::Duration::from_secs(device_response.interval as u64);
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_secs(device_response.expires_in as u64);

                loop {
                    if std::time::Instant::now() > deadline {
                        self.status = ConnectionStatus::Error("OAuth timeout".to_string());
                        return Err(anyhow::anyhow!("Device code expired"));
                    }

                    tokio::time::sleep(interval).await;

                    match client.poll_for_token(&device_response.device_code).await {
                        Ok(tokens) => {
                            storage.save_anthropic_oauth(tokens.clone())?;
                            self.credentials = Some(Credentials::OAuth(tokens));
                            self.status = ConnectionStatus::Connected;

                            self.chat.add_message(DisplayMessage::assistant_text(
                                "Successfully authenticated!",
                            ));
                            break;
                        }
                        Err(e) => {
                            let error = e.to_string();
                            if error.contains("authorization_pending") {
                                continue;
                            }
                            if error.contains("slow_down") {
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                continue;
                            }
                            self.status = ConnectionStatus::Error(error);
                            return Err(e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Create the agent with credentials
    fn create_agent(&mut self) -> Result<()> {
        let credentials = self
            .credentials
            .clone()
            .context("No credentials available")?;

        let client = AnthropicClient::new(
            credentials,
            self.config.general.model.clone(),
            self.config.general.max_tokens,
        )?
        .with_system_prompt(SYSTEM_PROMPT);

        let tools = ToolRegistry::new();

        // Create a permission handler that sends requests through a channel
        let permission_handler = Box::new(ChannelPermissionHandler::new());

        let agent = Agent::new(client, tools, permission_handler).with_system_prompt(SYSTEM_PROMPT);

        self.agent = Some(agent);

        Ok(())
    }

    /// Draw the UI
    fn draw(&mut self) -> Result<()> {
        let chat = &self.chat;
        let input = &self.input;
        let status = &self.status;
        let usage = self.usage;
        let model = &self.config.general.model;
        let show_tokens = self.config.ui.show_tokens;
        let permission_dialog = &self.permission_dialog;

        self.terminal.draw(|frame| {
            let size = frame.area();

            // Main layout
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),  // Status bar
                    Constraint::Min(10),    // Chat area
                    Constraint::Length(5),  // Input area
                ])
                .split(size);

            // Status bar
            let status_bar = StatusBar::new(APP_NAME, APP_VERSION, model, status)
                .usage(usage)
                .show_tokens(show_tokens);
            frame.render_widget(status_bar, chunks[0]);

            // Chat view
            frame.render_widget(chat.widget(), chunks[1]);

            // Input box
            frame.render_widget(input.widget(), chunks[2]);

            // Permission dialog (modal)
            if let Some(dialog) = permission_dialog {
                frame.render_widget(dialog.widget(), size);
            }
        })?;

        Ok(())
    }

    /// Handle permission dialog key events (non-async to avoid recursion)
    fn handle_permission_key(&mut self, key: KeyEvent) {
        if let Some(ref mut dialog) = self.permission_dialog {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    if let Some(tx) = self.permission_tx.take() {
                        let _ = tx.send(dialog.selected_response());
                    }
                    self.permission_dialog = None;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    if let Some(tx) = self.permission_tx.take() {
                        let _ = tx.send(PermissionResponse::Deny);
                    }
                    self.permission_dialog = None;
                }
                KeyCode::Char('a') => {
                    if let Some(tx) = self.permission_tx.take() {
                        let _ = tx.send(PermissionResponse::AllowForSession);
                    }
                    self.permission_dialog = None;
                }
                KeyCode::Tab | KeyCode::Right => {
                    dialog.next_action();
                }
                KeyCode::BackTab | KeyCode::Left => {
                    dialog.prev_action();
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Allow Ctrl+C to quit even from permission dialog
                    self.should_quit = true;
                }
                _ => {}
            }
        }
    }

    /// Handle a key event
    async fn handle_key_event(&mut self, key: KeyEvent) -> Result<()> {
        // Handle permission dialog if active
        if let Some(ref mut dialog) = self.permission_dialog {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    if let Some(tx) = self.permission_tx.take() {
                        let _ = tx.send(dialog.selected_response());
                    }
                    self.permission_dialog = None;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    if let Some(tx) = self.permission_tx.take() {
                        let _ = tx.send(PermissionResponse::Deny);
                    }
                    self.permission_dialog = None;
                }
                KeyCode::Char('a') => {
                    if let Some(tx) = self.permission_tx.take() {
                        let _ = tx.send(PermissionResponse::AllowForSession);
                    }
                    self.permission_dialog = None;
                }
                KeyCode::Tab | KeyCode::Right => {
                    dialog.next_action();
                }
                KeyCode::BackTab | KeyCode::Left => {
                    dialog.prev_action();
                }
                _ => {}
            }
            return Ok(());
        }

        // Global shortcuts
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return Ok(());
                }
                KeyCode::Char('l') => {
                    self.chat.clear();
                    return Ok(());
                }
                KeyCode::Enter => {
                    // Submit message
                    if !self.input.is_disabled() {
                        let content = self.input.submit();
                        if !content.trim().is_empty() {
                            self.send_message(content).await?;
                        }
                    }
                    return Ok(());
                }
                _ => {}
            }
        }

        // Input handling
        if !self.input.is_disabled() {
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
                    self.input.insert_newline();
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
        } else {
            // Scrolling while waiting
            match key.code {
                KeyCode::Up => self.chat.scroll_up(),
                KeyCode::Down => self.chat.scroll_down(),
                KeyCode::PageUp => self.chat.page_up(10),
                KeyCode::PageDown => self.chat.page_down(10),
                _ => {}
            }
        }

        Ok(())
    }

    /// Send a message to the agent
    async fn send_message(&mut self, content: String) -> Result<()> {
        // Add user message to chat
        self.chat.add_message(DisplayMessage::user(&content));

        // Disable input while processing
        self.input.set_mode(InputMode::Disabled);

        // Create event channel
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(100);

        // Take the agent temporarily
        let mut agent = self.agent.take().context("Agent not initialized")?;

        // Spawn agent processing task
        let content_clone = content.clone();
        let agent_handle = tokio::spawn(async move {
            let result = agent.process_message(&content_clone, event_tx).await;
            (agent, result)
        });

        // Process events
        let mut current_tool_calls: Vec<DisplayContent> = Vec::new();

        while let Some(event) = event_rx.recv().await {
            match event {
                AgentEvent::TextDelta(text) => {
                    self.chat.append_streaming_text(&text);
                    self.draw()?;
                }
                AgentEvent::TextComplete(_text) => {
                    // Text is already in streaming buffer
                }
                AgentEvent::ToolRequested { name, input, .. } => {
                    // Show permission dialog
                    let request = PermissionRequest {
                        tool_name: name.clone(),
                        params: input.clone(),
                        description: format_tool_description(&name, &input),
                        risk_level: get_risk_level(&name),
                    };

                    let (tx, rx) = oneshot::channel();
                    self.permission_dialog = Some(PermissionDialog::new(request));
                    self.permission_tx = Some(tx);

                    // Draw and wait for response
                    self.draw()?;

                    // Process key events until we get a permission response
                    loop {
                        if event::poll(std::time::Duration::from_millis(50))? {
                            if let Event::Key(key) = event::read()? {
                                self.handle_permission_key(key);
                                self.draw()?;
                            }
                        }

                        // Check if permission was granted
                        if self.permission_dialog.is_none() {
                            break;
                        }
                    }

                    // Get the response (will be received by the permission handler)
                    let _response = rx.await.unwrap_or(PermissionResponse::Deny);
                }
                AgentEvent::ToolExecuting { name, .. } => {
                    current_tool_calls.push(DisplayContent::ToolCall {
                        name: name.clone(),
                        summary: "Executing...".to_string(),
                        result: None,
                        is_error: false,
                    });
                }
                AgentEvent::ToolCompleted {
                    name,
                    result,
                    is_error,
                    ..
                } => {
                    // Update the tool call in our list
                    for call in &mut current_tool_calls {
                        if let DisplayContent::ToolCall {
                            name: call_name,
                            result: call_result,
                            is_error: call_error,
                            ..
                        } = call
                        {
                            if call_name == &name && call_result.is_none() {
                                *call_result = Some(result.clone());
                                *call_error = is_error;
                                break;
                            }
                        }
                    }
                }
                AgentEvent::ToolDenied { name, .. } => {
                    for call in &mut current_tool_calls {
                        if let DisplayContent::ToolCall {
                            name: call_name,
                            summary,
                            is_error: call_error,
                            ..
                        } = call
                        {
                            if call_name == &name {
                                *summary = "Denied by user".to_string();
                                *call_error = true;
                                break;
                            }
                        }
                    }
                }
                AgentEvent::Finished { usage } => {
                    self.usage = usage;
                }
                AgentEvent::Error(msg) => {
                    self.status = ConnectionStatus::Error(msg);
                }
            }
        }

        // Wait for agent task to complete
        let (returned_agent, result) = agent_handle.await?;
        self.agent = Some(returned_agent);

        // Finish streaming and add complete message
        if let Some(text) = self.chat.finish_streaming() {
            let mut content = vec![DisplayContent::Text(text)];
            content.extend(current_tool_calls);
            self.chat.add_message(DisplayMessage::assistant(content));
        } else if !current_tool_calls.is_empty() {
            self.chat
                .add_message(DisplayMessage::assistant(current_tool_calls));
        }

        // Re-enable input
        self.input.set_mode(InputMode::Insert);
        self.chat.enable_auto_scroll();

        // Handle any errors
        if let Err(e) = result {
            self.chat.add_message(DisplayMessage::assistant_text(format!(
                "Error: {}",
                e
            )));
        }

        Ok(())
    }

    /// Cleanup terminal
    fn cleanup(&mut self) -> Result<()> {
        disable_raw_mode().context("Failed to disable raw mode")?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )
        .context("Failed to cleanup terminal")?;
        self.terminal.show_cursor().context("Failed to show cursor")?;

        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Format a tool description for the permission dialog
fn format_tool_description(name: &str, params: &serde_json::Value) -> String {
    match name {
        "read_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Read file: {}", path)
        }
        "write_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Create file: {}", path)
        }
        "edit_file" => {
            let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let edits = params
                .get("edits")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            format!("Edit file: {} ({} changes)", path, edits)
        }
        "shell" => {
            let command = params.get("command").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Execute: {}", command)
        }
        "fetch_url" => {
            let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Fetch: {}", url)
        }
        _ => format!("{}: {:?}", name, params),
    }
}

/// Get the risk level for a tool
fn get_risk_level(name: &str) -> RiskLevel {
    match name {
        "read_file" | "fetch_url" => RiskLevel::Low,
        "write_file" | "edit_file" => RiskLevel::Medium,
        "shell" => RiskLevel::High,
        _ => RiskLevel::Medium,
    }
}

/// Permission handler that uses channels to communicate with the UI
struct ChannelPermissionHandler;

impl ChannelPermissionHandler {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl PermissionHandler for ChannelPermissionHandler {
    async fn request_permission(&self, _request: PermissionRequest) -> PermissionResponse {
        // The actual permission handling is done in the main event loop
        // This handler is a placeholder that the UI will intercept
        // For now, return Allow (the UI should handle this properly)
        PermissionResponse::Allow
    }
}
