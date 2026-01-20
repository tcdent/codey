# Client Integration Guide

This document provides guidance for developers implementing codey-server clients in any programming language.

## Overview

The codey-server exposes a WebSocket API that allows clients to:
- Send messages to an AI coding assistant
- Receive streaming responses (text, thinking, tool execution)
- Approve or deny tool executions
- Query conversation history and session state

## Connection

Connect to the server using a standard WebSocket connection:

```
ws://127.0.0.1:9999
```

The default port is `9999` but can be configured via the `--listen` flag when starting the server.

## Protocol

All messages are JSON objects with a `type` field that indicates the message type. The protocol uses tagged unions (discriminated unions) for type safety.

## Message Types

### Client → Server

#### SendMessage
Send a message to the agent.

```json
{
  "type": "SendMessage",
  "content": "Hello, how are you?",
  "agent_id": null
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `"SendMessage"` | Yes | Message type discriminator |
| `content` | `string` | Yes | The message content |
| `agent_id` | `int \| null` | No | Optional agent ID for multi-agent sessions |

#### ToolDecision
Approve or deny a pending tool execution.

```json
{
  "type": "ToolDecision",
  "call_id": "call_abc123",
  "approved": true
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `"ToolDecision"` | Yes | Message type discriminator |
| `call_id` | `string` | Yes | The call_id from ToolAwaitingApproval |
| `approved` | `bool` | Yes | `true` to approve, `false` to deny |

#### Cancel
Cancel the current operation (interrupt streaming, cancel running tools).

```json
{
  "type": "Cancel"
}
```

#### GetHistory
Request conversation history.

```json
{
  "type": "GetHistory"
}
```

Response will be a `History` message.

#### GetState
Request current session state (useful for reconnection).

```json
{
  "type": "GetState"
}
```

Response will be a `State` message.

#### Ping
Keep connection alive.

```json
{
  "type": "Ping"
}
```

Response will be a `Pong` message.

---

### Server → Client

#### Connected
Sent immediately after connection is established.

```json
{
  "type": "Connected",
  "session_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session_id` | `string` | Unique identifier for this session |

#### TextDelta
Streaming text content from the agent.

```json
{
  "type": "TextDelta",
  "agent_id": 0,
  "content": "Hello! I'm doing well, thank you for asking."
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agent_id` | `int` | ID of the agent producing this text |
| `content` | `string` | Text content delta (append to previous) |

#### ThinkingDelta
Streaming thinking/reasoning from the agent (extended thinking mode).

```json
{
  "type": "ThinkingDelta",
  "agent_id": 0,
  "content": "Let me consider the user's question..."
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agent_id` | `int` | ID of the agent |
| `content` | `string` | Thinking content delta |

#### ToolRequest
Agent is requesting tool execution. This is informational; the actual approval flow happens via `ToolAwaitingApproval`.

```json
{
  "type": "ToolRequest",
  "agent_id": 0,
  "calls": [
    {
      "call_id": "call_abc123",
      "name": "Read",
      "params": {"file_path": "/path/to/file.txt"},
      "background": false
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agent_id` | `int` | ID of the agent requesting tools |
| `calls` | `ToolCallInfo[]` | List of tool calls requested |

#### ToolAwaitingApproval
A tool execution requires user approval (didn't pass server-side auto-approve filters).

```json
{
  "type": "ToolAwaitingApproval",
  "agent_id": 0,
  "call_id": "call_abc123",
  "name": "Bash",
  "params": {"command": "ls -la"},
  "background": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `agent_id` | `int` | ID of the agent |
| `call_id` | `string` | Unique identifier for this tool call |
| `name` | `string` | Name of the tool |
| `params` | `object` | Parameters passed to the tool |
| `background` | `bool` | Whether this tool runs in the background |

**Action Required:** Send a `ToolDecision` message to approve or deny.

#### ToolStarted
Tool execution has started (after approval or auto-approval).

```json
{
  "type": "ToolStarted",
  "agent_id": 0,
  "call_id": "call_abc123",
  "name": "Read"
}
```

#### ToolDelta
Streaming output from tool execution.

```json
{
  "type": "ToolDelta",
  "agent_id": 0,
  "call_id": "call_abc123",
  "content": "File contents..."
}
```

#### ToolCompleted
Tool execution completed successfully.

```json
{
  "type": "ToolCompleted",
  "agent_id": 0,
  "call_id": "call_abc123",
  "content": "Full file contents here..."
}
```

#### ToolError
Tool execution failed or was denied.

```json
{
  "type": "ToolError",
  "agent_id": 0,
  "call_id": "call_abc123",
  "error": "File not found"
}
```

#### Finished
Agent has finished processing the current turn.

```json
{
  "type": "Finished",
  "agent_id": 0,
  "usage": {
    "output_tokens": 150,
    "context_tokens": 4500,
    "cache_creation_tokens": 0,
    "cache_read_tokens": 3200
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `usage.output_tokens` | `int` | Cumulative output tokens across session |
| `usage.context_tokens` | `int` | Current context window size |
| `usage.cache_creation_tokens` | `int` | Cache creation tokens in last request |
| `usage.cache_read_tokens` | `int` | Cache read tokens in last request |

#### Retrying
Agent is retrying after a transient error.

```json
{
  "type": "Retrying",
  "agent_id": 0,
  "attempt": 1,
  "error": "Rate limit exceeded"
}
```

#### History
Response to `GetHistory` request.

```json
{
  "type": "History",
  "messages": [
    {
      "role": "user",
      "content": "Hello!",
      "timestamp": "2024-01-15T10:30:00Z"
    },
    {
      "role": "assistant",
      "content": "Hi there!",
      "timestamp": "2024-01-15T10:30:01Z"
    }
  ]
}
```

#### State
Response to `GetState` request.

```json
{
  "type": "State",
  "agents": [
    {
      "id": 0,
      "name": null,
      "is_streaming": false
    }
  ],
  "pending_approvals": [
    {
      "agent_id": 0,
      "call_id": "call_abc123",
      "name": "Bash",
      "params": {"command": "ls"}
    }
  ]
}
```

#### Pong
Response to `Ping` request.

```json
{
  "type": "Pong"
}
```

#### Error
An error occurred.

```json
{
  "type": "Error",
  "message": "Something went wrong",
  "fatal": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `message` | `string` | Error description |
| `fatal` | `bool` | If `true`, the session is no longer usable |

---

## Typical Message Flow

### Simple Chat

```
Client                          Server
  |                               |
  |-- SendMessage --------------->|
  |                               |
  |<-- TextDelta ----------------|  (multiple)
  |<-- TextDelta ----------------|
  |<-- TextDelta ----------------|
  |<-- Finished -----------------|
  |                               |
```

### Chat with Tool Execution

```
Client                          Server
  |                               |
  |-- SendMessage --------------->|
  |                               |
  |<-- TextDelta ----------------|
  |<-- ToolRequest --------------|  (informational)
  |<-- ToolAwaitingApproval -----|  (needs decision)
  |                               |
  |-- ToolDecision (approve) ---->|
  |                               |
  |<-- ToolStarted --------------|
  |<-- ToolDelta ----------------|  (streaming output)
  |<-- ToolCompleted ------------|
  |<-- TextDelta ----------------|  (agent continues)
  |<-- Finished -----------------|
  |                               |
```

---

## Implementation Checklist

When implementing a client, ensure you handle:

- [ ] **Connection lifecycle**: Connect, maintain, reconnect on failure
- [ ] **Message parsing**: Parse JSON with type discriminator
- [ ] **Streaming text**: Accumulate `TextDelta` messages
- [ ] **Tool approval**: Respond to `ToolAwaitingApproval` with `ToolDecision`
- [ ] **Turn completion**: Wait for `Finished` before accepting new input
- [ ] **Error handling**: Handle both `Error` messages and WebSocket errors
- [ ] **Keepalive**: Send `Ping` periodically to maintain connection

## Reference Implementations

- **Python**: [`clients/python/`](./python/) - Full async client with Pydantic schemas

---

## Server Configuration

The server loads configuration from `~/.config/codey/config.toml`. Tool filter rules defined there determine which tools are auto-approved or auto-denied. Tools not matching any filter are sent to the client via `ToolAwaitingApproval`.

Example filter configuration:
```toml
[tools.Read]
allow = [".*"]  # Auto-approve all Read calls

[tools.Bash]
deny = ["rm -rf.*", "sudo.*"]  # Auto-deny dangerous commands
```

---

## Versioning

The protocol is versioned with the codey-server. Breaking changes will be noted in release notes. The `Connected` message may include version information in future releases.
