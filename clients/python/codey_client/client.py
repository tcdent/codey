"""
Codey WebSocket client implementation.

Provides an async client for interacting with the codey-server.
"""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator, Callable
from contextlib import asynccontextmanager
from typing import Any

import websockets
from websockets.asyncio.client import ClientConnection

from .schemas import (
    Cancel,
    ClientMessage,
    Connected,
    Error,
    Finished,
    GetHistory,
    GetState,
    Ping,
    Pong,
    SendMessage,
    ServerMessage,
    TextDelta,
    ThinkingDelta,
    ToolAwaitingApproval,
    ToolCompleted,
    ToolDecision,
    ToolDelta,
    ToolError,
    ToolRequest,
    ToolStarted,
    parse_server_message,
)


class CodeyClient:
    """Async WebSocket client for codey-server.

    Example:
        ```python
        async with CodeyClient("ws://localhost:9999") as client:
            async for msg in client.chat("Hello, how are you?"):
                if isinstance(msg, TextDelta):
                    print(msg.content, end="", flush=True)
        ```
    """

    def __init__(
        self,
        url: str = "ws://127.0.0.1:9999",
        auto_approve: bool = False,
        on_approval_request: Callable[[ToolAwaitingApproval], bool] | None = None,
    ):
        """Initialize the client.

        Args:
            url: WebSocket URL of the codey-server.
            auto_approve: If True, automatically approve all tool requests.
            on_approval_request: Callback for tool approval requests. Return True
                to approve, False to deny. If not provided and auto_approve is False,
                tools will be denied by default.
        """
        self.url = url
        self.auto_approve = auto_approve
        self.on_approval_request = on_approval_request
        self._ws: ClientConnection | None = None
        self._session_id: str | None = None
        self._receive_task: asyncio.Task[None] | None = None
        self._message_queue: asyncio.Queue[ServerMessage] = asyncio.Queue()

    @property
    def session_id(self) -> str | None:
        """The current session ID, or None if not connected."""
        return self._session_id

    @property
    def is_connected(self) -> bool:
        """Whether the client is currently connected."""
        return self._ws is not None and self._ws.state.name == "OPEN"

    async def connect(self) -> None:
        """Connect to the codey-server.

        Raises:
            ConnectionError: If connection fails.
        """
        try:
            self._ws = await websockets.connect(self.url)
        except Exception as e:
            raise ConnectionError(f"Failed to connect to {self.url}: {e}") from e

        # Start background receive task
        self._receive_task = asyncio.create_task(self._receive_loop())

        # Wait for Connected message
        msg = await self._message_queue.get()
        if isinstance(msg, Connected):
            self._session_id = msg.session_id
        elif isinstance(msg, Error):
            raise ConnectionError(f"Server error: {msg.message}")
        else:
            raise ConnectionError(f"Unexpected message: {msg}")

    async def disconnect(self) -> None:
        """Disconnect from the server."""
        if self._receive_task:
            self._receive_task.cancel()
            try:
                await self._receive_task
            except asyncio.CancelledError:
                pass
            self._receive_task = None

        if self._ws:
            await self._ws.close()
            self._ws = None

        self._session_id = None

    async def __aenter__(self) -> CodeyClient:
        """Async context manager entry."""
        await self.connect()
        return self

    async def __aexit__(self, *args: Any) -> None:
        """Async context manager exit."""
        await self.disconnect()

    async def _receive_loop(self) -> None:
        """Background task to receive messages from the server."""
        assert self._ws is not None
        try:
            async for raw_msg in self._ws:
                if isinstance(raw_msg, bytes):
                    raw_msg = raw_msg.decode("utf-8")
                data = json.loads(raw_msg)
                msg = parse_server_message(data)
                await self._message_queue.put(msg)
        except websockets.ConnectionClosed:
            # Put an error message to signal connection closed
            await self._message_queue.put(
                Error(message="Connection closed", fatal=True)
            )

    async def _send(self, msg: ClientMessage) -> None:
        """Send a message to the server."""
        if not self._ws:
            raise ConnectionError("Not connected")
        await self._ws.send(msg.model_dump_json())

    async def send_message(
        self, content: str, agent_id: int | None = None
    ) -> None:
        """Send a message to the agent.

        Args:
            content: The message content.
            agent_id: Optional agent ID for multi-agent sessions.
        """
        await self._send(SendMessage(content=content, agent_id=agent_id))

    async def approve_tool(self, call_id: str) -> None:
        """Approve a pending tool execution.

        Args:
            call_id: The call_id of the tool to approve.
        """
        await self._send(ToolDecision(call_id=call_id, approved=True))

    async def deny_tool(self, call_id: str) -> None:
        """Deny a pending tool execution.

        Args:
            call_id: The call_id of the tool to deny.
        """
        await self._send(ToolDecision(call_id=call_id, approved=False))

    async def cancel(self) -> None:
        """Cancel the current operation."""
        await self._send(Cancel())

    async def get_history(self) -> None:
        """Request conversation history. Response will be in the message stream."""
        await self._send(GetHistory())

    async def get_state(self) -> None:
        """Request current session state. Response will be in the message stream."""
        await self._send(GetState())

    async def ping(self) -> None:
        """Send a ping to keep the connection alive."""
        await self._send(Ping())

    async def receive(self, timeout: float | None = None) -> ServerMessage | None:
        """Receive the next message from the server.

        Args:
            timeout: Optional timeout in seconds. If None, wait indefinitely.

        Returns:
            The next server message, or None if timeout reached.
        """
        try:
            if timeout is not None:
                return await asyncio.wait_for(
                    self._message_queue.get(), timeout=timeout
                )
            return await self._message_queue.get()
        except asyncio.TimeoutError:
            return None

    async def chat(
        self, message: str, agent_id: int | None = None
    ) -> AsyncIterator[ServerMessage]:
        """Send a message and yield all responses until the turn is complete.

        This is the main high-level API for chatting with the agent. It handles
        tool approval requests based on the client's configuration.

        Args:
            message: The message to send.
            agent_id: Optional agent ID for multi-agent sessions.

        Yields:
            Server messages (TextDelta, ThinkingDelta, ToolStarted, etc.)
            until a Finished or fatal Error is received.
        """
        await self.send_message(message, agent_id)

        while True:
            msg = await self.receive()
            if msg is None:
                break

            # Handle tool approval requests
            if isinstance(msg, ToolAwaitingApproval):
                if self.auto_approve:
                    await self.approve_tool(msg.call_id)
                elif self.on_approval_request:
                    approved = self.on_approval_request(msg)
                    if approved:
                        await self.approve_tool(msg.call_id)
                    else:
                        await self.deny_tool(msg.call_id)
                else:
                    # Default: deny
                    await self.deny_tool(msg.call_id)

            yield msg

            # Check for turn completion
            if isinstance(msg, Finished):
                break
            if isinstance(msg, Error) and msg.fatal:
                break

    async def stream_text(
        self, message: str, agent_id: int | None = None
    ) -> AsyncIterator[str]:
        """Send a message and yield only the text content.

        This is a convenience method that filters the chat stream to only
        yield text deltas, making it easy to print or accumulate the response.

        Args:
            message: The message to send.
            agent_id: Optional agent ID for multi-agent sessions.

        Yields:
            Text content strings from TextDelta messages.
        """
        async for msg in self.chat(message, agent_id):
            if isinstance(msg, TextDelta):
                yield msg.content


@asynccontextmanager
async def connect(
    url: str = "ws://127.0.0.1:9999",
    auto_approve: bool = False,
    on_approval_request: Callable[[ToolAwaitingApproval], bool] | None = None,
) -> AsyncIterator[CodeyClient]:
    """Connect to codey-server as an async context manager.

    Example:
        ```python
        async with connect("ws://localhost:9999", auto_approve=True) as client:
            async for text in client.stream_text("What files are in the current directory?"):
                print(text, end="", flush=True)
        ```

    Args:
        url: WebSocket URL of the codey-server.
        auto_approve: If True, automatically approve all tool requests.
        on_approval_request: Callback for tool approval requests.

    Yields:
        A connected CodeyClient instance.
    """
    client = CodeyClient(url, auto_approve, on_approval_request)
    await client.connect()
    try:
        yield client
    finally:
        await client.disconnect()
