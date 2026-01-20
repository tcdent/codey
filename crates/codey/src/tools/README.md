# Tools System

This directory contains the tool execution system for Codey. Tools are defined as pipelines of effect handlers that can be composed, approved, and executed.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         ToolExecutor                            │
│  - Manages pending/active tool pipelines                        │
│  - Handles approval flow                                        │
│  - Spawns handlers in separate tasks (won't be dropped)         │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                        ToolPipeline                             │
│  - Chain of EffectHandlers                                      │
│  - Supports .then(), .await_approval(), .finally()              │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                       EffectHandler                             │
│  - Async trait: async fn call(self: Box<Self>) -> Step          │
│  - Returns Step::Continue, Output, Delta, Delegate, Error       │
└─────────────────────────────────────────────────────────────────┘
```

## Key Files

- `exec.rs` - ToolExecutor, WaitingFor state machine, event handling
- `pipeline.rs` - ToolPipeline, Step enum, EffectHandler trait, Tool trait
- `handlers.rs` - Reusable effect handlers (Shell, ReadFile, WriteFile, etc.)
- `io.rs` - Low-level I/O operations (run_shell, read_file, etc.)
- `impls/` - Individual tool implementations

## Adding a New Tool

### 1. Create the Tool Implementation

Create a new file in `src/tools/impls/your_tool.rs`:

```rust
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{handlers, Tool, ToolPipeline};
use crate::impl_base_block;
use crate::transcript::{
    render_approval_prompt, render_prefix, render_result, 
    Block, BlockType, Status,
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

// =============================================================================
// Custom Block (for UI rendering)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YourToolBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
    #[serde(default)]
    pub background: bool,
}

impl YourToolBlock {
    pub fn new(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        params: serde_json::Value,
        background: bool,
    ) -> Self {
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
impl Block for YourToolBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        // Extract params for display
        let some_param = self.params["some_param"].as_str().unwrap_or("");

        // Format: your_tool(param_value)
        let spans = vec![
            self.render_status(),
            render_prefix(self.background),
            Span::styled("your_tool", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(some_param, Style::default().fg(Color::White)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ];
        lines.push(Line::from(spans));

        // Approval prompt if pending
        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        // Output if completed
        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 10));
        }

        // Denied message
        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }

    fn tool_name(&self) -> Option<&str> {
        Some(&self.tool_name)
    }

    fn params(&self) -> Option<&serde_json::Value> {
        Some(&self.params)
    }
}

// =============================================================================
// Tool Definition
// =============================================================================

pub struct YourTool;

#[derive(Debug, Deserialize)]
struct YourToolParams {
    some_param: String,
    optional_param: Option<String>,
}

impl YourTool {
    pub const NAME: &'static str = "mcp_your_tool";
}

impl Tool for YourTool {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn description(&self) -> &'static str {
        "Description shown to the LLM. Be clear about what this tool does."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "some_param": {
                    "type": "string",
                    "description": "Description of this parameter"
                },
                "optional_param": {
                    "type": "string",
                    "description": "Optional parameter description"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in background. Returns immediately with a task_id."
                }
            },
            "required": ["some_param"]
        })
    }

    fn compose(&self, params: serde_json::Value) -> ToolPipeline {
        // Parse and validate params
        let parsed: YourToolParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolPipeline::error(format!("Invalid params: {}", e)),
        };

        // Build the pipeline
        ToolPipeline::new()
            .await_approval()  // Pause for user approval
            .then(handlers::YourHandler {
                param: parsed.some_param,
            })
    }

    fn create_block(
        &self,
        call_id: &str,
        params: serde_json::Value,
        background: bool,
    ) -> Box<dyn Block> {
        Box::new(YourToolBlock::new(call_id, self.name(), params, background))
    }
}
```

### 2. Create a Handler (if needed)

Add to `src/tools/handlers.rs`:

```rust
pub struct YourHandler {
    pub param: String,
}

#[async_trait::async_trait]
impl EffectHandler for YourHandler {
    async fn call(self: Box<Self>) -> Step {
        // Do the actual work
        match do_something(&self.param).await {
            Ok(result) => Step::Output(result),
            Err(e) => Step::Error(e.to_string()),
        }
    }
}
```

### 3. Register the Tool

In `src/tools/impls/mod.rs`:

```rust
mod your_tool;
pub use your_tool::YourTool;
```

In `src/tools/mod.rs`, add to `ToolRegistry::new()`:

```rust
registry.register(Arc::new(YourTool));
```

### 4. Add Config Support (for auto-approve filters)

In `src/config.rs`, add to `ToolsConfig`:

```rust
pub struct ToolsConfig {
    // ... existing fields ...
    pub your_tool: ToolFilterConfig,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            // ... existing fields ...
            your_tool: ToolFilterConfig::default(),
        }
    }
}

impl ToolsConfig {
    pub fn filters(&self) -> HashMap<String, ToolFilterConfig> {
        // ... existing mappings ...
        map.insert(YourTool::NAME.to_string(), self.your_tool.clone());
        map
    }
}
```

In `src/tool_filter.rs`, update `primary_param()`:

```rust
fn primary_param(tool_name: &str) -> &'static str {
    match tool_name {
        // ... existing matches ...
        x if x == YourTool::NAME => "some_param",
        _ => "command",
    }
}
```

Don't forget to add the import at the top of `tool_filter.rs`.

## Pipeline Patterns

### Basic Tool (no approval needed)
```rust
ToolPipeline::new()
    .then(handlers::DoSomething { ... })
```

### Tool with Approval
```rust
ToolPipeline::new()
    .then(handlers::ValidateInput { ... })  // Pre-approval validation
    .await_approval()                        // User decides here
    .then(handlers::DoSomething { ... })    // Only runs if approved
```

### Tool with Cleanup
```rust
ToolPipeline::new()
    .then(handlers::SetupPreview { ... })
    .await_approval()
    .then(handlers::ApplyChanges { ... })
    .finally(handlers::ClosePreview)  // Always runs, even on deny/error
```

### Tool that Delegates to App Layer
```rust
// In handler:
async fn call(self: Box<Self>) -> Step {
    Step::Delegate(Effect::IdeOpen { path: self.path, line: None, column: None })
}
```

The app receives a `ToolEvent::Delegate` and sends back a result via oneshot channel.

## Step Variants

- `Step::Continue` - Proceed to next handler, no output
- `Step::Output(String)` - Set final output, proceed to next handler
- `Step::Delta(String)` - Emit streaming content (for long-running tools)
- `Step::Delegate(Effect)` - Ask app layer to do something
- `Step::AwaitApproval` - Pause for user approval (use `.await_approval()` instead)
- `Step::Error(String)` - Abort pipeline with error

## Background Task Considerations

When `background: true` is passed:
1. Tool runs in background, returns task_id immediately
2. User can check status with `list_background_tasks`
3. User retrieves result with `get_background_task(task_id)`
4. Output is stored in `ActivePipeline.output` until retrieved

The executor spawns handlers in separate tokio tasks, so they won't be lost if the main select! loop moves on to other work.

## Testing

Add tests in your tool file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ToolExecutor, ToolRegistry, ToolCall, ToolDecision};

    #[tokio::test]
    async fn test_your_tool() {
        let mut registry = ToolRegistry::empty();
        registry.register(std::sync::Arc::new(YourTool));
        let mut executor = ToolExecutor::new(registry);

        executor.enqueue(vec![ToolCall {
            agent_id: 0,
            call_id: "test".to_string(),
            name: YourTool::NAME.to_string(),
            params: json!({ "some_param": "test_value" }),
            decision: ToolDecision::Approve,  // Skip approval for tests
            background: false,
        }]);

        if let Some(ToolEvent::Completed { content, .. }) = executor.next().await {
            assert!(content.contains("expected"));
        } else {
            panic!("Expected Completed event");
        }
    }
}
```

## UI Rendering Notes

- Use `render_status()` for the spinner/checkmark prefix
- Use `render_prefix(background)` for the `[bg]` indicator
- Use `render_approval_prompt()` for the `[y]es [n]o` prompt
- Use `render_result(&text, max_lines)` for output with truncation
- Tool names in UI should strip the `mcp_` prefix for cleaner display
