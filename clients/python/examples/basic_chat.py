#!/usr/bin/env python3
"""
Basic chat example for codey-client.

Usage:
    # Start the server first:
    codey-server --listen 127.0.0.1:9999

    # Then run this script:
    python examples/basic_chat.py

    # Or with auto-approve:
    python examples/basic_chat.py --auto-approve
"""

from __future__ import annotations

import argparse
import asyncio
import sys

from codey_client import (
    CodeyClient,
    Error,
    Finished,
    Retrying,
    TextDelta,
    ThinkingDelta,
    ToolAwaitingApproval,
    ToolCompleted,
    ToolError,
    ToolStarted,
)


def approval_prompt(tool: ToolAwaitingApproval) -> bool:
    """Prompt the user to approve or deny a tool execution."""
    print(f"\n[Tool Request] {tool.name}")
    print(f"  Parameters: {tool.params}")
    while True:
        response = input("  Approve? [y/n]: ").strip().lower()
        if response in ("y", "yes"):
            return True
        if response in ("n", "no"):
            return False
        print("  Please enter 'y' or 'n'")


async def chat_loop(client: CodeyClient) -> None:
    """Run an interactive chat loop."""
    print("Connected to codey-server!")
    print("Type your message and press Enter. Type 'quit' to exit.\n")

    while True:
        try:
            user_input = input("You: ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\nGoodbye!")
            break

        if not user_input:
            continue
        if user_input.lower() in ("quit", "exit", "q"):
            print("Goodbye!")
            break

        print("Assistant: ", end="", flush=True)

        try:
            async for msg in client.chat(user_input):
                if isinstance(msg, TextDelta):
                    print(msg.content, end="", flush=True)

                elif isinstance(msg, ThinkingDelta):
                    # Optionally show thinking (usually hidden)
                    pass

                elif isinstance(msg, ToolStarted):
                    print(f"\n[Executing: {msg.name}]", flush=True)

                elif isinstance(msg, ToolCompleted):
                    # Tool output (often long, truncate for display)
                    output = msg.content[:200] + "..." if len(msg.content) > 200 else msg.content
                    print(f"[Completed: {output}]", flush=True)
                    print("Assistant: ", end="", flush=True)

                elif isinstance(msg, ToolError):
                    print(f"\n[Tool Error: {msg.error}]", flush=True)

                elif isinstance(msg, Retrying):
                    print(f"\n[Retrying: attempt {msg.attempt}, {msg.error}]", flush=True)

                elif isinstance(msg, Finished):
                    print(f"\n[Tokens: {msg.usage.output_tokens}]\n")

                elif isinstance(msg, Error):
                    print(f"\n[Error: {msg.message}]")
                    if msg.fatal:
                        print("Fatal error, disconnecting.")
                        return

        except Exception as e:
            print(f"\nError: {e}")


async def main() -> None:
    parser = argparse.ArgumentParser(description="Chat with codey-server")
    parser.add_argument(
        "--url",
        default="ws://127.0.0.1:9999",
        help="WebSocket URL (default: ws://127.0.0.1:9999)",
    )
    parser.add_argument(
        "--auto-approve",
        action="store_true",
        help="Automatically approve all tool requests",
    )
    args = parser.parse_args()

    # Set up approval callback unless auto-approve is enabled
    on_approval = None if args.auto_approve else approval_prompt

    try:
        async with CodeyClient(
            url=args.url,
            auto_approve=args.auto_approve,
            on_approval_request=on_approval,
        ) as client:
            await chat_loop(client)
    except ConnectionError as e:
        print(f"Connection failed: {e}", file=sys.stderr)
        print("Make sure codey-server is running.", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
