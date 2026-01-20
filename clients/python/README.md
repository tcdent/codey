# Codey Python Client

A Python WebSocket client for interacting with `codey-server`.

## Installation

```bash
# From this directory
pip install -e .

# Or with dev dependencies
pip install -e ".[dev]"
```

## Quick Start

```python
import asyncio
from codey_client import connect

async def main():
    # Connect to codey-server
    async with connect("ws://localhost:9999", auto_approve=True) as client:
        print(f"Connected: {client.session_id}")

        # Stream a response
        async for text in client.stream_text("What is 2 + 2?"):
            print(text, end="", flush=True)
        print()

asyncio.run(main())
```

## Usage

### Basic Chat

```python
from codey_client import connect, TextDelta, Finished

async with connect() as client:
    async for msg in client.chat("Hello!"):
        if isinstance(msg, TextDelta):
            print(msg.content, end="")
        elif isinstance(msg, Finished):
            print(f"\n\nTokens: {msg.usage.output_tokens}")
```

### Tool Approval

By default, tools that don't match server-side filters require approval:

```python
from codey_client import connect, ToolAwaitingApproval

def approve_handler(tool: ToolAwaitingApproval) -> bool:
    """Custom approval logic."""
    # Approve read-only tools, deny others
    return tool.name in ["Read", "Glob", "Grep"]

async with connect(on_approval_request=approve_handler) as client:
    async for msg in client.chat("List files in the current directory"):
        ...
```

Or auto-approve everything (use with caution):

```python
async with connect(auto_approve=True) as client:
    ...
```

### Low-Level API

```python
client = CodeyClient("ws://localhost:9999")
await client.connect()

# Send a message
await client.send_message("Hello!")

# Receive messages manually
while True:
    msg = await client.receive(timeout=30.0)
    if msg is None:
        break
    print(msg)

await client.disconnect()
```

## Message Types

All message types are Pydantic models with full type hints.

### Client → Server

- `SendMessage` - Send a message to the agent
- `ToolDecision` - Approve or deny a tool
- `Cancel` - Cancel current operation
- `GetHistory` - Request conversation history
- `GetState` - Request session state
- `Ping` - Keep connection alive

### Server → Client

- `Connected` - Session established
- `TextDelta` - Streaming text from agent
- `ThinkingDelta` - Streaming thinking/reasoning
- `ToolRequest` - Agent requesting tools
- `ToolAwaitingApproval` - Tool needs approval
- `ToolStarted` - Tool execution started
- `ToolDelta` - Streaming tool output
- `ToolCompleted` - Tool completed
- `ToolError` - Tool failed
- `Finished` - Turn complete with usage stats
- `Retrying` - Agent retrying after error
- `History` - Conversation history
- `State` - Session state
- `Pong` - Response to ping
- `Error` - Error occurred

## Development

```bash
# Install dev dependencies
pip install -e ".[dev]"

# Run type checking
mypy codey_client

# Run linting
ruff check codey_client

# Run tests
pytest
```
