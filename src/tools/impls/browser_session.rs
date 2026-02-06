//! Browser session tools
//!
//! Persistent browser sessions that agents can open, interact with, and close.
//! Mirrors the list_/get_ convention from background_tasks and agent_management.

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::impl_tool_block;
use crate::transcript::{
    render_approval_prompt, render_prefix, render_result, Block, BlockType, Status, ToolBlock,
};

// =============================================================================
// browser_open
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserOpenBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl BrowserOpenBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }
}

#[typetag::serde]
impl Block for BrowserOpenBlock {
    impl_tool_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let url = self.params["url"].as_str().unwrap_or("");
        let session = self.params["session_name"].as_str().unwrap_or("");

        let mut spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("browser_open", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(url, Style::default().fg(Color::Blue)),
        ];
        if !session.is_empty() {
            spans.push(Span::styled(", ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(session, Style::default().fg(Color::White)));
        }
        spans.push(Span::styled(")", Style::default().fg(Color::DarkGray)));
        lines.push(Line::from(spans));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
        }
        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> { Some(&self.call_id) }
    fn tool_name(&self) -> Option<&str> { Some(&self.tool_name) }
    fn params(&self) -> Option<&serde_json::Value> { Some(&self.params) }
}

pub struct BrowserOpenTool;

#[derive(Debug, Deserialize)]
struct BrowserOpenParams {
    url: String,
    session_name: Option<String>,
}

impl BrowserOpenTool {
    pub const NAME: &'static str = "mcp_browser_open";
}

impl Tool for BrowserOpenTool {
    fn name(&self) -> &'static str { Self::NAME }

    fn description(&self) -> &'static str {
        "Open a persistent browser session and navigate to a URL. Returns the page content as \
         readable markdown. The session stays alive for subsequent actions (click, fill, navigate). \
         Use browser_close to end the session when done."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to navigate to"
                },
                "session_name": {
                    "type": "string",
                    "description": "Optional name for the session. Auto-generated if omitted. \
                                    If a session with this name exists, it will navigate to the new URL."
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id."
                }
            },
            "required": ["url"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: BrowserOpenParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        ToolPipeline::new()
            .await_approval()
            .then(handlers::BrowserOpen {
                url: parsed.url,
                session_name: parsed.session_name,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(BrowserOpenBlock::new(call_id, self.name(), params, background))
    }
}

// =============================================================================
// browser_action
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserActionBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl BrowserActionBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }
}

#[typetag::serde]
impl Block for BrowserActionBlock {
    impl_tool_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let session = self.params["session_name"].as_str().unwrap_or("");
        let action = self.params["action"].as_str().unwrap_or("");

        lines.push(Line::from(vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("browser_action", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(session, Style::default().fg(Color::White)),
            Span::styled(", ", Style::default().fg(Color::DarkGray)),
            Span::styled(action, Style::default().fg(Color::Yellow)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
        }
        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> { Some(&self.call_id) }
    fn tool_name(&self) -> Option<&str> { Some(&self.tool_name) }
    fn params(&self) -> Option<&serde_json::Value> { Some(&self.params) }
}

pub struct BrowserActionTool;

#[derive(Debug, Deserialize)]
struct BrowserActionParams {
    session_name: String,
    action: String,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    amount: Option<u32>,
    #[serde(default)]
    ms: Option<u64>,
    #[serde(default)]
    script: Option<String>,
}

impl BrowserActionTool {
    pub const NAME: &'static str = "mcp_browser_action";
}

impl Tool for BrowserActionTool {
    fn name(&self) -> &'static str { Self::NAME }

    fn description(&self) -> &'static str {
        "Perform an action on a browser session: navigate, click, fill, select, scroll, \
         back, forward, wait, or evaluate JavaScript. Returns updated page content after \
         the action completes and the page settles."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "session_name": {
                    "type": "string",
                    "description": "Name of the browser session (from browser_open)"
                },
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "fill", "select", "scroll", "back", "forward", "wait", "evaluate"],
                    "description": "The action to perform"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for click/fill/select actions"
                },
                "value": {
                    "type": "string",
                    "description": "Value for fill/select actions"
                },
                "url": {
                    "type": "string",
                    "description": "URL for navigate action"
                },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down"],
                    "description": "Direction for scroll action"
                },
                "amount": {
                    "type": "integer",
                    "description": "Pixels to scroll (default: 500)"
                },
                "ms": {
                    "type": "integer",
                    "description": "Milliseconds for wait action (max: 30000)"
                },
                "script": {
                    "type": "string",
                    "description": "JavaScript code for evaluate action"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id."
                }
            },
            "required": ["session_name", "action"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: BrowserActionParams = match serde_json::from_value(params.clone()) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        // Build the action-specific params as a JSON value
        let action_params = json!({
            "selector": parsed.selector,
            "value": parsed.value,
            "url": parsed.url,
            "direction": parsed.direction,
            "amount": parsed.amount,
            "ms": parsed.ms,
            "script": parsed.script,
        });

        ToolPipeline::new()
            .await_approval()
            .then(handlers::BrowserAction {
                session_name: parsed.session_name,
                action: parsed.action,
                params: action_params,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(BrowserActionBlock::new(call_id, self.name(), params, background))
    }
}

// =============================================================================
// browser_snapshot
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSnapshotBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl BrowserSnapshotBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }
}

#[typetag::serde]
impl Block for BrowserSnapshotBlock {
    impl_tool_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let session = self.params["session_name"].as_str().unwrap_or("");

        lines.push(Line::from(vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("browser_snapshot", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(session, Style::default().fg(Color::White)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
        }
        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> { Some(&self.call_id) }
    fn tool_name(&self) -> Option<&str> { Some(&self.tool_name) }
    fn params(&self) -> Option<&serde_json::Value> { Some(&self.params) }
}

pub struct BrowserSnapshotTool;

#[derive(Debug, Deserialize)]
struct BrowserSnapshotParams {
    session_name: String,
}

impl BrowserSnapshotTool {
    pub const NAME: &'static str = "mcp_browser_snapshot";
}

impl Tool for BrowserSnapshotTool {
    fn name(&self) -> &'static str { Self::NAME }

    fn description(&self) -> &'static str {
        "Get a fresh snapshot of the current page content in a browser session. \
         Returns the readable content as markdown. Use this to re-read a page after \
         waiting for dynamic content to load."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "session_name": {
                    "type": "string",
                    "description": "Name of the browser session"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id."
                }
            },
            "required": ["session_name"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: BrowserSnapshotParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        ToolPipeline::new()
            .await_approval()
            .then(handlers::BrowserSnapshot {
                session_name: parsed.session_name,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(BrowserSnapshotBlock::new(call_id, self.name(), params, background))
    }
}

// =============================================================================
// browser_list_sessions
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserListSessionsBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl BrowserListSessionsBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }
}

#[typetag::serde]
impl Block for BrowserListSessionsBlock {
    impl_tool_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        lines.push(Line::from(vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("browser_list_sessions", Style::default().fg(Color::Magenta)),
            Span::styled("()", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
        }
        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> { Some(&self.call_id) }
    fn tool_name(&self) -> Option<&str> { Some(&self.tool_name) }
    fn params(&self) -> Option<&serde_json::Value> { Some(&self.params) }
}

pub struct BrowserListSessionsTool;

impl BrowserListSessionsTool {
    pub const NAME: &'static str = "mcp_browser_list_sessions";
}

impl Tool for BrowserListSessionsTool {
    fn name(&self) -> &'static str { Self::NAME }

    fn description(&self) -> &'static str {
        "List all active browser sessions. Shows session names, current URLs, \
         and idle time. Use browser_close to clean up sessions you no longer need."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn compose(&self, _params: serde_json::Value) -> ToolPipeline {
        ToolPipeline::new()
            .await_approval()
            .then(handlers::BrowserListSessions)
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(BrowserListSessionsBlock::new(call_id, self.name(), params, background))
    }
}

// =============================================================================
// browser_close
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserCloseBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl BrowserCloseBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value, background: bool) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
            background,
        }
    }
}

#[typetag::serde]
impl Block for BrowserCloseBlock {
    impl_tool_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let session = self.params["session_name"].as_str().unwrap_or("");

        lines.push(Line::from(vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("browser_close", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(session, Style::default().fg(Color::White)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
        }
        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> { Some(&self.call_id) }
    fn tool_name(&self) -> Option<&str> { Some(&self.tool_name) }
    fn params(&self) -> Option<&serde_json::Value> { Some(&self.params) }
}

pub struct BrowserCloseTool;

#[derive(Debug, Deserialize)]
struct BrowserCloseParams {
    session_name: String,
}

impl BrowserCloseTool {
    pub const NAME: &'static str = "mcp_browser_close";
}

impl Tool for BrowserCloseTool {
    fn name(&self) -> &'static str { Self::NAME }

    fn description(&self) -> &'static str {
        "Close a browser session and release its resources. Always close sessions when \
         you're done to avoid leaving Chromium processes running."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "session_name": {
                    "type": "string",
                    "description": "Name of the browser session to close"
                }
            },
            "required": ["session_name"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        let parsed: BrowserCloseParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        ToolPipeline::new()
            .await_approval()
            .then(handlers::BrowserClose {
                session_name: parsed.session_name,
            })
    }

    fn create_block(&self, call_id: &str, params: serde_json::Value, background: bool) -> Box<dyn Block> {
        Box::new(BrowserCloseBlock::new(call_id, self.name(), params, background))
    }
}
