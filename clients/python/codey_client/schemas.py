"""
Pydantic schemas for the Codey WebSocket protocol.

These schemas mirror the Rust types defined in crates/codey-server/src/protocol.rs
"""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field


# ============================================================================
# Supporting Types
# ============================================================================


class Usage(BaseModel):
    """Token usage statistics from the agent."""

    output_tokens: int = Field(description="Cumulative output tokens across the session")
    context_tokens: int = Field(description="Current context window size")
    cache_creation_tokens: int = Field(description="Cache creation tokens in last request")
    cache_read_tokens: int = Field(description="Cache read tokens in last request")


class ToolCallInfo(BaseModel):
    """Information about a tool call."""

    call_id: str = Field(description="Unique identifier for this tool call")
    name: str = Field(description="Name of the tool being called")
    params: dict[str, Any] = Field(description="Parameters passed to the tool")
    background: bool = Field(description="Whether this tool runs in the background")


class HistoryMessage(BaseModel):
    """A message in the conversation history."""

    role: str = Field(description="Message role: 'user', 'assistant', or 'tool'")
    content: str = Field(description="Message content")
    timestamp: str | None = Field(default=None, description="ISO 8601 timestamp")


class AgentInfo(BaseModel):
    """Information about an agent in the session."""

    id: int = Field(description="Agent ID")
    name: str | None = Field(default=None, description="Optional agent name")
    is_streaming: bool = Field(description="Whether the agent is currently streaming")


class PendingApproval(BaseModel):
    """A tool call awaiting user approval."""

    agent_id: int = Field(description="ID of the agent that requested the tool")
    call_id: str = Field(description="Unique identifier for this tool call")
    name: str = Field(description="Name of the tool")
    params: dict[str, Any] = Field(description="Parameters passed to the tool")


# ============================================================================
# Client → Server Messages
# ============================================================================


class SendMessage(BaseModel):
    """Send a message to the agent."""

    type: Literal["SendMessage"] = "SendMessage"
    content: str = Field(description="The message content to send")
    agent_id: int | None = Field(
        default=None, description="Optional agent ID for multi-agent sessions"
    )


class ToolDecision(BaseModel):
    """Approve or deny a pending tool execution."""

    type: Literal["ToolDecision"] = "ToolDecision"
    call_id: str = Field(description="The call_id of the tool to approve/deny")
    approved: bool = Field(description="True to approve, False to deny")


class Cancel(BaseModel):
    """Cancel current operation."""

    type: Literal["Cancel"] = "Cancel"


class GetHistory(BaseModel):
    """Request conversation history."""

    type: Literal["GetHistory"] = "GetHistory"


class GetState(BaseModel):
    """Request current session state."""

    type: Literal["GetState"] = "GetState"


class Ping(BaseModel):
    """Ping to keep connection alive."""

    type: Literal["Ping"] = "Ping"


# Union type for all client messages
ClientMessage = SendMessage | ToolDecision | Cancel | GetHistory | GetState | Ping


# ============================================================================
# Server → Client Messages
# ============================================================================


class Connected(BaseModel):
    """Session established."""

    type: Literal["Connected"] = "Connected"
    session_id: str = Field(description="Unique session identifier")


class TextDelta(BaseModel):
    """Streaming text from agent."""

    type: Literal["TextDelta"] = "TextDelta"
    agent_id: int = Field(description="ID of the agent producing this text")
    content: str = Field(description="Text content delta")


class ThinkingDelta(BaseModel):
    """Streaming thinking/reasoning from agent."""

    type: Literal["ThinkingDelta"] = "ThinkingDelta"
    agent_id: int = Field(description="ID of the agent")
    content: str = Field(description="Thinking content delta")


class ToolRequest(BaseModel):
    """Agent requesting tool execution."""

    type: Literal["ToolRequest"] = "ToolRequest"
    agent_id: int = Field(description="ID of the agent requesting tools")
    calls: list[ToolCallInfo] = Field(description="List of tool calls requested")


class ToolAwaitingApproval(BaseModel):
    """Tool awaiting user approval."""

    type: Literal["ToolAwaitingApproval"] = "ToolAwaitingApproval"
    agent_id: int = Field(description="ID of the agent")
    call_id: str = Field(description="Unique identifier for this tool call")
    name: str = Field(description="Name of the tool")
    params: dict[str, Any] = Field(description="Parameters passed to the tool")
    background: bool = Field(description="Whether this tool runs in the background")


class ToolStarted(BaseModel):
    """Tool execution started."""

    type: Literal["ToolStarted"] = "ToolStarted"
    agent_id: int = Field(description="ID of the agent")
    call_id: str = Field(description="Unique identifier for this tool call")
    name: str = Field(description="Name of the tool")


class ToolDelta(BaseModel):
    """Streaming output from tool execution."""

    type: Literal["ToolDelta"] = "ToolDelta"
    agent_id: int = Field(description="ID of the agent")
    call_id: str = Field(description="Unique identifier for this tool call")
    content: str = Field(description="Output content delta")


class ToolCompleted(BaseModel):
    """Tool execution completed successfully."""

    type: Literal["ToolCompleted"] = "ToolCompleted"
    agent_id: int = Field(description="ID of the agent")
    call_id: str = Field(description="Unique identifier for this tool call")
    content: str = Field(description="Final output content")


class ToolError(BaseModel):
    """Tool execution failed or was denied."""

    type: Literal["ToolError"] = "ToolError"
    agent_id: int = Field(description="ID of the agent")
    call_id: str = Field(description="Unique identifier for this tool call")
    error: str = Field(description="Error message")


class Finished(BaseModel):
    """Agent finished processing (turn complete)."""

    type: Literal["Finished"] = "Finished"
    agent_id: int = Field(description="ID of the agent")
    usage: Usage = Field(description="Token usage statistics")


class Retrying(BaseModel):
    """Agent is retrying after transient error."""

    type: Literal["Retrying"] = "Retrying"
    agent_id: int = Field(description="ID of the agent")
    attempt: int = Field(description="Retry attempt number")
    error: str = Field(description="Error that caused the retry")


class History(BaseModel):
    """Conversation history response."""

    type: Literal["History"] = "History"
    messages: list[HistoryMessage] = Field(description="List of history messages")


class State(BaseModel):
    """Session state response."""

    type: Literal["State"] = "State"
    agents: list[AgentInfo] = Field(description="List of agents in the session")
    pending_approvals: list[PendingApproval] = Field(
        description="List of tool calls awaiting approval"
    )


class Pong(BaseModel):
    """Pong response to Ping."""

    type: Literal["Pong"] = "Pong"


class Error(BaseModel):
    """Error occurred."""

    type: Literal["Error"] = "Error"
    message: str = Field(description="Error message")
    fatal: bool = Field(description="If true, the session is no longer usable")


# Union type for all server messages
ServerMessage = (
    Connected
    | TextDelta
    | ThinkingDelta
    | ToolRequest
    | ToolAwaitingApproval
    | ToolStarted
    | ToolDelta
    | ToolCompleted
    | ToolError
    | Finished
    | Retrying
    | History
    | State
    | Pong
    | Error
)

# Type discriminator for parsing server messages
SERVER_MESSAGE_TYPES: dict[str, type[BaseModel]] = {
    "Connected": Connected,
    "TextDelta": TextDelta,
    "ThinkingDelta": ThinkingDelta,
    "ToolRequest": ToolRequest,
    "ToolAwaitingApproval": ToolAwaitingApproval,
    "ToolStarted": ToolStarted,
    "ToolDelta": ToolDelta,
    "ToolCompleted": ToolCompleted,
    "ToolError": ToolError,
    "Finished": Finished,
    "Retrying": Retrying,
    "History": History,
    "State": State,
    "Pong": Pong,
    "Error": Error,
}


def parse_server_message(data: dict[str, Any]) -> ServerMessage:
    """Parse a server message from a dictionary.

    Args:
        data: Dictionary containing the message data with a 'type' field.

    Returns:
        The parsed server message.

    Raises:
        ValueError: If the message type is unknown.
    """
    msg_type = data.get("type")
    if msg_type not in SERVER_MESSAGE_TYPES:
        raise ValueError(f"Unknown server message type: {msg_type}")
    return SERVER_MESSAGE_TYPES[msg_type].model_validate(data)
