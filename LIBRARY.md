# Using Codey as a Library

Codey can be used as a library to create AI agents with custom system prompts in your Rust projects.

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

Alternatively, clone the codey repository and reference the patched genai locally:

```toml
[patch.crates-io]
genai = { path = "../codey/lib/genai" }
```

Note: Run `make patch` in the codey repository first to download and patch the dependencies.

## Basic Usage

```rust
use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, ToolRegistry};

#[tokio::main]
async fn main() {
    // Create an agent with a custom system prompt (no tools)
    let mut agent = Agent::new(
        AgentRuntimeConfig::default(),
        "You are a helpful assistant. Answer questions concisely.",
        None, // OAuth credentials (uses ANTHROPIC_API_KEY env var)
        ToolRegistry::empty(),
    );

    // Send a message
    agent.send_request("What is the capital of France?", RequestMode::Normal);

    // Process streaming responses
    while let Some(step) = agent.next().await {
        match step {
            AgentStep::TextDelta(text) => print!("{}", text),
            AgentStep::ThinkingDelta(_) => { /* extended thinking output */ }
            AgentStep::Finished { usage } => {
                println!("\n\nTokens used: {}", usage.output_tokens);
                break;
            }
            AgentStep::Error(e) => {
                eprintln!("Error: {}", e);
                break;
            }
            _ => {}
        }
    }
}
```

## Public API

### `Agent`

The main agent type that handles conversations with Claude.

```rust
// Create a new agent
let mut agent = Agent::new(
    config,           // AgentRuntimeConfig
    system_prompt,    // &str
    oauth,            // Option<OAuthCredentials>
    tools,            // ToolRegistry
);

// Send a message
agent.send_request("Hello!", RequestMode::Normal);

// Get streaming responses
while let Some(step) = agent.next().await {
    // Handle AgentStep variants
}
```

### `AgentRuntimeConfig`

Configuration for the agent:

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

Events emitted during agent processing:

- `TextDelta(String)` - Streaming text output
- `ThinkingDelta(String)` - Extended thinking output
- `Finished { usage: Usage }` - Processing complete
- `Error(String)` - Error occurred
- `Retrying { attempt, error }` - Retrying after error
- `ToolRequest(Vec<ToolCall>)` - Tools requested (not used with empty registry)
- `CompactionDelta(String)` - Context compaction output

### `RequestMode`

Controls agent behavior:

- `RequestMode::Normal` - Standard conversation
- `RequestMode::Compaction` - Context compaction mode

### `ToolRegistry`

For library usage, create an empty registry:

```rust
let tools = ToolRegistry::empty();
```

### `Usage`

Token usage statistics returned in `AgentStep::Finished`:

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

## Example: Interactive Chat

```rust
use codey::{Agent, AgentRuntimeConfig, AgentStep, RequestMode, ToolRegistry};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut agent = Agent::new(
        AgentRuntimeConfig::default(),
        "You are a helpful assistant.",
        None,
        ToolRegistry::empty(),
    );

    loop {
        print!("> ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        if input == "quit" {
            break;
        }

        agent.send_request(input, RequestMode::Normal);

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

## Limitations

The library exposes a minimal API focused on creating agents with custom system prompts. The built-in tools (file operations, shell, web search, etc.) are specific to the Codey binary and its UI, so they are not exposed in the library API.

If you need tool capabilities, you can:
1. Implement tool-like behavior in your system prompt
2. Parse structured output from the agent
3. Build your own tool execution layer outside of codey
