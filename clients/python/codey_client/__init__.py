"""
Codey Python Client

A WebSocket client for interacting with codey-server.

Example:
    ```python
    import asyncio
    from codey_client import connect

    async def main():
        async with connect("ws://localhost:9999", auto_approve=True) as client:
            async for text in client.stream_text("Hello!"):
                print(text, end="", flush=True)
            print()

    asyncio.run(main())
    ```
"""

from .client import CodeyClient, connect
from .schemas import (
    # Supporting types
    AgentInfo,
    HistoryMessage,
    PendingApproval,
    ToolCallInfo,
    Usage,
    # Client messages
    Cancel,
    ClientMessage,
    GetHistory,
    GetState,
    Ping,
    SendMessage,
    ToolDecision,
    # Server messages
    Connected,
    Error,
    Finished,
    History,
    Pong,
    Retrying,
    ServerMessage,
    State,
    TextDelta,
    ThinkingDelta,
    ToolAwaitingApproval,
    ToolCompleted,
    ToolDelta,
    ToolError,
    ToolRequest,
    ToolStarted,
    # Utilities
    parse_server_message,
)

__version__ = "0.1.0"

__all__ = [
    # Client
    "CodeyClient",
    "connect",
    # Supporting types
    "Usage",
    "ToolCallInfo",
    "HistoryMessage",
    "AgentInfo",
    "PendingApproval",
    # Client messages
    "ClientMessage",
    "SendMessage",
    "ToolDecision",
    "Cancel",
    "GetHistory",
    "GetState",
    "Ping",
    # Server messages
    "ServerMessage",
    "Connected",
    "TextDelta",
    "ThinkingDelta",
    "ToolRequest",
    "ToolAwaitingApproval",
    "ToolStarted",
    "ToolDelta",
    "ToolCompleted",
    "ToolError",
    "Finished",
    "Retrying",
    "History",
    "State",
    "Pong",
    "Error",
    # Utilities
    "parse_server_message",
]
