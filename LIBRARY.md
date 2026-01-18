# Using Codey as a Library

Codey can be used as a library to build custom AI agents with tool capabilities in your own Rust projects.

## Installation

Add codey to your `Cargo.toml`:

```toml
[dependencies]
codey = { git = "https://github.com/tcdent/codey" }
tokio = { version = "1", features = ["full"] }
```

### Patched Dependencies

Codey uses a patched version of the `genai` crate. To use Codey as a library, you'll need to apply the same patch in your project's `Cargo.toml`:

```toml
[patch.crates-io]
genai = { git = "https://github.com/tcdent/codey", branch = "main" }
```

Alternatively, you can clone the codey repository and reference the patched genai locally:

```toml
[patch.crates-io]
genai = { path = "../codey/lib/genai" }
```

Note: Run `make patch` in the codey repository first to download and patch the dependencies.

## Basic Usage

```rust
use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, ToolRegistry};
use codey::tools::{ReadFileTool, WriteFileTool, ShellTool};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // Build a custom tool registry with specific tools
    let mut tools = ToolRegistry::empty();
    tools.register(Arc::new(ReadFileTool));
    tools.register(Arc::new(WriteFileTool));
    tools.register(Arc::new(ShellTool::new()));

    // Create an agent with a custom system prompt
    let config = AgentRuntimeConfig::default();
    let mut agent = Agent::new(
        config,
        "You are a helpful coding assistant. Help users with their programming tasks.",
        None, // OAuth credentials (None for API key auth)
        tools,
    );

    // Send a message to the agent
    agent.send_request("What files are in the current directory?", RequestMode::Normal);

    // Process streaming responses
    while let Some(step) = agent.next().await {
        match step {
            AgentStep::TextDelta(text) => {
                print!("{}", text);
            }
            AgentStep::ThinkingDelta(thinking) => {
                // Handle extended thinking output if desired
                // print!("[thinking] {}", thinking);
            }
            AgentStep::ToolRequest(calls) => {
                // Tools need approval before execution
                // In a real application, you'd implement approval logic here
                println!("\nTool requests: {:?}", calls.iter().map(|c| &c.name).collect::<Vec<_>>());

                // For this example, we'll just note that tools were requested
                // and break out of the loop
                break;
            }
            AgentStep::Finished { usage } => {
                println!("\n\nCompleted. Output tokens: {}", usage.output_tokens);
                break;
            }
            AgentStep::Error(err) => {
                eprintln!("Error: {}", err);
                break;
            }
            AgentStep::Retrying { attempt, error } => {
                eprintln!("Retrying (attempt {}): {}", attempt, error);
            }
            _ => {}
        }
    }
}
```

## Core Types

### `Agent`

The main agent type that handles conversations with the LLM.

```rust
// Create a new agent
let agent = Agent::new(config, system_prompt, oauth, tools);

// Send a message
agent.send_request("Hello!", RequestMode::Normal);

// Get streaming responses
while let Some(step) = agent.next().await {
    // Handle AgentStep variants
}

// Submit tool results after execution
agent.submit_tool_result(&call_id, result_string);
```

### `AgentRuntimeConfig`

Configuration for the agent runtime:

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

### `ToolRegistry`

Manages the tools available to the agent:

```rust
// Empty registry - add tools manually
let mut tools = ToolRegistry::empty();
tools.register(Arc::new(ReadFileTool));

// Full registry with all tools
let tools = ToolRegistry::new();

// Read-only tools (no write_file, edit_file)
let tools = ToolRegistry::read_only();

// Sub-agent tools (read-only, no spawn_agent)
let tools = ToolRegistry::subagent();
```

### `AgentStep`

Enum representing events during agent processing:

- `TextDelta(String)` - Streaming text output
- `ThinkingDelta(String)` - Extended thinking output
- `ToolRequest(Vec<ToolCall>)` - Tools need approval/execution
- `Finished { usage: Usage }` - Processing complete
- `Error(String)` - Error occurred
- `Retrying { attempt, error }` - Retrying after error
- `CompactionDelta(String)` - Context compaction summary

### `RequestMode`

Controls agent behavior for a request:

- `RequestMode::Normal` - Standard conversation with tool access
- `RequestMode::Compaction` - Context compaction mode (no tools)

## Available Tools

Import tools from `codey::tools`:

```rust
use codey::tools::{
    ReadFileTool,       // Read file contents
    WriteFileTool,      // Create/overwrite files
    EditFileTool,       // Make precise edits to files
    ShellTool,          // Execute shell commands
    FetchUrlTool,       // Fetch URL content (raw)
    FetchHtmlTool,      // Fetch and extract HTML content
    WebSearchTool,      // Web search
    OpenFileTool,       // Signal file to open in IDE
    SpawnAgentTool,     // Spawn background agents
    ListBackgroundTasksTool,
    GetBackgroundTaskTool,
};
```

## Tool Execution

When the agent requests tools, you receive a `ToolRequest` with `Vec<ToolCall>`. Each tool call needs:

1. **Approval** - Set `call.decision` to `ToolDecision::Approve` or `ToolDecision::Deny`
2. **Execution** - Run the tool and get results
3. **Result submission** - Call `agent.submit_tool_result(&call_id, result)`

For a complete tool execution implementation, see `src/tools/exec.rs` in the codey source.

## Authentication

Codey supports two authentication methods:

1. **API Key** (default) - Set `ANTHROPIC_API_KEY` environment variable
2. **OAuth** - Pass `OAuthCredentials` to `Agent::new()`

```rust
// API key auth (reads from ANTHROPIC_API_KEY env var)
let agent = Agent::new(config, prompt, None, tools);

// OAuth auth
let oauth = OAuthCredentials { /* ... */ };
let agent = Agent::new(config, prompt, Some(oauth), tools);
```

## Example: Simple Chat Agent

```rust
use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, ToolRegistry};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut agent = Agent::new(
        AgentRuntimeConfig::default(),
        "You are a helpful assistant. Answer questions concisely.",
        None,
        ToolRegistry::empty(), // No tools - just chat
    );

    loop {
        // Get user input
        print!("> ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input == "quit" {
            break;
        }

        // Send to agent
        agent.send_request(input, RequestMode::Normal);

        // Stream response
        while let Some(step) = agent.next().await {
            match step {
                AgentStep::TextDelta(text) => print!("{}", text),
                AgentStep::Finished { .. } => {
                    println!();
                    break;
                }
                AgentStep::Error(e) => {
                    eprintln!("\nError: {}", e);
                    break;
                }
                _ => {}
            }
        }
    }

    Ok(())
}
```
