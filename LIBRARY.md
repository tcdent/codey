# Using Codey as a Library

Codey can be used as a library to create AI agents with custom system prompts and tools in your Rust projects.

## Installation

Add codey to your `Cargo.toml`:

```toml
[dependencies]
codey = { git = "https://github.com/tcdent/codey", default-features = false }
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

### Lightweight Builds

By default, codey includes the full CLI with TUI rendering, IDE integrations, and web extraction features. For library usage, disable the default features to get a minimal dependency footprint:

```toml
# Minimal library (recommended for integrations)
codey = { git = "https://github.com/tcdent/codey", default-features = false }

# Full CLI features (includes ratatui, crossterm, chromiumoxide, etc.)
codey = { git = "https://github.com/tcdent/codey" }
```

With `default-features = false`, codey pulls only the core dependencies needed for the agent:
- `genai` - LLM client
- `tokio` - Async runtime
- `serde`/`serde_json` - Serialization
- `reqwest` - HTTP client
- Error handling and utilities

### Patched Dependencies

Codey uses a patched version of the `genai` crate. To use Codey as a library, you'll need to apply the same patch in your project's `Cargo.toml`:

```toml
[patch.crates-io]
genai = { git = "https://github.com/tcdent/codey", branch = "main" }
```

Alternatively, clone the codey repository and reference the patched genai locally:

```toml
[patch.crates-io]
genai = { path = "../codey/lib/genai" }
```

Note: Run `make patch` in the codey repository first to download and patch the dependencies.

## Basic Usage (No Tools)

```rust
use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, ToolRegistry};

#[tokio::main]
async fn main() {
    let mut agent = Agent::new(
        AgentRuntimeConfig::default(),
        "You are a helpful assistant.",
        None, // uses ANTHROPIC_API_KEY env var
        ToolRegistry::empty(),
    );

    agent.send_request("What is the capital of France?", RequestMode::Normal);

    while let Some(step) = agent.next().await {
        match step {
            AgentStep::TextDelta(text) => print!("{}", text),
            AgentStep::Finished { .. } => break,
            AgentStep::Error(e) => {
                eprintln!("Error: {}", e);
                break;
            }
            _ => {}
        }
    }
}
```

## Custom Tools

You can define custom tools using `SimpleTool` and handle their execution yourself.

### Defining Tools

```rust
use codey::{SimpleTool, ToolRegistry};
use serde_json::json;
use std::sync::Arc;

// Define a tool
let weather_tool = SimpleTool::new(
    "get_weather",
    "Get the current weather for a location",
    json!({
        "type": "object",
        "properties": {
            "location": {
                "type": "string",
                "description": "City name, e.g. 'San Francisco'"
            }
        },
        "required": ["location"]
    }),
);

// Register tools
let mut tools = ToolRegistry::empty();
tools.register(Arc::new(weather_tool));
```

### Handling Tool Calls

When the LLM wants to use a tool, you'll receive an `AgentStep::ToolRequest`. You must execute the tool and submit the result back to the agent:

```rust
use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, SimpleTool, ToolCall, ToolRegistry};
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Set up tools
    let weather_tool = SimpleTool::new(
        "get_weather",
        "Get the current weather for a location",
        json!({
            "type": "object",
            "properties": {
                "location": { "type": "string" }
            },
            "required": ["location"]
        }),
    );

    let mut tools = ToolRegistry::empty();
    tools.register(Arc::new(weather_tool));

    // Create agent with tools
    let mut agent = Agent::new(
        AgentRuntimeConfig::default(),
        "You are a helpful assistant with access to weather data.",
        None,
        tools,
    );

    agent.send_request("What's the weather in Paris?", RequestMode::Normal);

    loop {
        match agent.next().await {
            Some(AgentStep::TextDelta(text)) => print!("{}", text),

            Some(AgentStep::ToolRequest(calls)) => {
                // Handle each tool call
                for call in calls {
                    let result = execute_tool(&call);
                    agent.submit_tool_result(&call.call_id, result);
                }
                // Continue processing after submitting results
            }

            Some(AgentStep::Finished { .. }) => {
                println!();
                break;
            }

            Some(AgentStep::Error(e)) => {
                eprintln!("Error: {}", e);
                break;
            }

            None => break,
            _ => {}
        }
    }
}

fn execute_tool(call: &ToolCall) -> String {
    match call.name.as_str() {
        "get_weather" => {
            let location = call.params["location"].as_str().unwrap_or("unknown");
            // Your actual implementation here
            format!("Weather in {}: Sunny, 22°C", location)
        }
        _ => format!("Unknown tool: {}", call.name),
    }
}
```

### `ToolCall` Structure

When you receive a tool request, each `ToolCall` contains:

```rust
pub struct ToolCall {
    pub call_id: String,           // Unique ID for this call (use with submit_tool_result)
    pub name: String,              // Tool name
    pub params: serde_json::Value, // Parameters from the LLM
    // ... other fields
}
```

## Public API Reference

### `Agent`

The main agent type for conversations with Claude.

```rust
let mut agent = Agent::new(config, system_prompt, oauth, tools);
agent.send_request("Hello!", RequestMode::Normal);
while let Some(step) = agent.next().await { /* ... */ }
agent.submit_tool_result(&call_id, result);
```

### `AgentRuntimeConfig`

```rust
let config = AgentRuntimeConfig {
    model: "claude-sonnet-4-20250514".to_string(),
    max_tokens: 8192,
    thinking_budget: 2_000,
    max_retries: 5,
    compaction_thinking_budget: 8_000,
};

// Or use defaults
let config = AgentRuntimeConfig::default();
```

### `AgentStep`

Events emitted during processing:

- `TextDelta(String)` - Streaming text output
- `ThinkingDelta(String)` - Extended thinking output
- `ToolRequest(Vec<ToolCall>)` - LLM wants to use tools
- `Finished { usage: Usage }` - Processing complete
- `Error(String)` - Error occurred
- `Retrying { attempt, error }` - Retrying after error

### `SimpleTool`

Define a tool for the LLM to use:

```rust
let tool = SimpleTool::new(
    "tool_name",           // Name the LLM will use
    "Description of tool", // Help the LLM understand when to use it
    json!({ /* JSON Schema for parameters */ }),
);
```

### `ToolRegistry`

Manage available tools:

```rust
let mut tools = ToolRegistry::empty();
tools.register(Arc::new(my_tool));
```

### `Usage`

Token usage statistics:

```rust
pub struct Usage {
    pub output_tokens: u32,
    pub context_tokens: u32,
    pub cache_creation_tokens: u32,
    pub cache_read_tokens: u32,
}
```

## Authentication

Set the `ANTHROPIC_API_KEY` environment variable:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
```
