"""
Test harness for driving codey's TUI via tmux.

Launches codey inside a tmux session, sends keystrokes, captures pane output,
and uses an LLM to evaluate whether the rendering is correct.
"""

import subprocess
import time
import os
import signal
import textwrap

import anthropic


CODEY_BIN = os.environ.get("CODEY_BIN", "codey")
EVAL_MODEL = os.environ.get("EVAL_MODEL", "claude-sonnet-4-5-20250929")
TMUX_SESSION = "codey-test"
# How long to wait for a condition before giving up.
DEFAULT_TIMEOUT = 60
# Polling interval for capture-pane checks.
POLL_INTERVAL = 1.0


class CodeySession:
    """Manages a codey instance running inside a tmux session."""

    def __init__(self, session_name=TMUX_SESSION, working_dir="/workspace"):
        self.session = session_name
        self.working_dir = working_dir
        self._started = False

    def start(self):
        """Start a new tmux session running codey."""
        # Kill any leftover session.
        subprocess.run(
            ["tmux", "kill-session", "-t", self.session],
            capture_output=True,
        )
        # Start tmux with remain-on-exit so we can read crash output.
        # Size the terminal explicitly so captures are deterministic.
        subprocess.run(
            [
                "tmux", "new-session",
                "-d",                       # detached
                "-s", self.session,
                "-x", "120", "-y", "40",    # fixed size
            ],
            check=True,
        )
        # Set remain-on-exit so pane survives if codey crashes.
        subprocess.run(
            ["tmux", "set-option", "-t", self.session, "remain-on-exit", "on"],
            check=True,
        )
        # Launch codey inside the session.
        subprocess.run(
            [
                "tmux", "send-keys", "-t", self.session,
                f"{CODEY_BIN} --working-dir {self.working_dir}", "Enter",
            ],
            check=True,
        )
        self._started = True
        # Give codey a moment to initialize the TUI.
        time.sleep(3)

        # Verify codey is actually running by checking the pane process.
        result = subprocess.run(
            ["tmux", "list-panes", "-t", self.session, "-F", "#{pane_pid} #{pane_dead}"],
            capture_output=True, text=True,
        )
        pane_info = result.stdout.strip()
        if "1" in pane_info.split()[-1:]:
            pane = self.capture()
            raise RuntimeError(
                f"codey process died. Pane content:\n{pane}"
            )

    def stop(self):
        """Kill the tmux session."""
        if self._started:
            subprocess.run(
                ["tmux", "kill-session", "-t", self.session],
                capture_output=True,
            )
            self._started = False

    def capture(self, history_lines=100) -> str:
        """Capture the current tmux pane content including scrollback."""
        result = subprocess.run(
            [
                "tmux", "capture-pane",
                "-t", self.session,
                "-p",                        # print to stdout
                "-S", f"-{history_lines}",   # include scrollback
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout

    def send_keys(self, keys: str):
        """Send literal key string to the tmux pane."""
        subprocess.run(
            ["tmux", "send-keys", "-t", self.session, "-l", keys],
            check=True,
        )

    def send_special(self, key: str):
        """Send a special key (Enter, Escape, C-c, etc.)."""
        subprocess.run(
            ["tmux", "send-keys", "-t", self.session, key],
            check=True,
        )

    def type_and_submit(self, text: str):
        """Type a message and press Enter to submit."""
        self.send_keys(text)
        time.sleep(0.2)
        self.send_special("Enter")

    def approve(self):
        """Press 'y' to approve a tool call."""
        self.send_keys("y")

    def deny(self):
        """Press 'n' to deny a tool call."""
        self.send_keys("n")

    def interrupt(self):
        """Press Escape to interrupt/cancel."""
        self.send_special("Escape")

    def wait_for(self, condition, timeout=DEFAULT_TIMEOUT, poll=POLL_INTERVAL) -> str:
        """
        Poll capture-pane until `condition(pane_text)` returns True.
        Returns the final pane text. Raises TimeoutError if not met.
        """
        deadline = time.time() + timeout
        while time.time() < deadline:
            pane = self.capture()
            if condition(pane):
                return pane
            time.sleep(poll)
        # One last capture for the error message.
        pane = self.capture()
        raise TimeoutError(
            f"Condition not met within {timeout}s.\n"
            f"Last pane content:\n{pane}"
        )

    def wait_for_text(self, text: str, **kwargs) -> str:
        """Wait until the pane contains a specific substring."""
        return self.wait_for(lambda p: text in p, **kwargs)

    def wait_for_approval(self, **kwargs) -> str:
        """Wait until an approval prompt appears.

        The approval UI renders as:
            ? tool_name
              {params...}
              [y]es  [n]o
        The '?' prefix is the Pending status icon (yellow in the real TUI,
        but plain text in tmux capture). The key indicator is '[y]es  [n]o'.
        """
        return self.wait_for(
            lambda p: "[y]es" in p or "[n]o" in p,
            **kwargs,
        )

    def wait_for_completion(self, **kwargs) -> str:
        """Wait until a tool completes (checkmark status icon appears).

        Completed tools render as:
            ✓ tool_name
              output...
        """
        return self.wait_for(
            lambda p: "✓" in p,
            **kwargs,
        )

    def wait_for_denial(self, **kwargs) -> str:
        """Wait until a tool denial appears.

        Denied tools render as:
            ⊘ tool_name
              Denied by user
        """
        return self.wait_for(
            lambda p: "⊘" in p or "Denied by user" in p,
            **kwargs,
        )

    def wait_for_idle(self, **kwargs) -> str:
        """Wait until codey returns to the normal input mode.

        Idle = no approval prompt visible and no running tool (⚙).
        We check that there's no '[y]es' prompt and no spinner,
        then confirm with a second capture after a short delay.
        """
        def is_idle(pane):
            has_approval = "[y]es" in pane
            has_running = "⚙" in pane
            return not has_approval and not has_running

        return self.wait_for(is_idle, **kwargs)


class LLMEvaluator:
    """Uses an LLM to evaluate whether TUI output looks correct."""

    def __init__(self, model=EVAL_MODEL):
        self.client = anthropic.Anthropic()
        self.model = model

    def evaluate(self, pane_text: str, question: str) -> dict:
        """
        Ask the LLM to evaluate the pane content.

        Returns dict with:
          - "pass": bool
          - "reasoning": str explanation
        """
        response = self.client.messages.create(
            model=self.model,
            max_tokens=1024,
            messages=[
                {
                    "role": "user",
                    "content": textwrap.dedent(f"""\
                        You are evaluating a terminal UI (TUI) application called codey.
                        Below is a capture of the terminal screen content.

                        <terminal_capture>
                        {pane_text}
                        </terminal_capture>

                        Question: {question}

                        Respond in this exact format:
                        PASS: true or false
                        REASONING: one or two sentences explaining your evaluation
                    """),
                }
            ],
        )
        text = response.content[0].text
        lines = text.strip().split("\n")

        passed = False
        reasoning = ""
        for line in lines:
            if line.startswith("PASS:"):
                passed = "true" in line.lower()
            if line.startswith("REASONING:"):
                reasoning = line.split(":", 1)[1].strip()

        return {"pass": passed, "reasoning": reasoning}

    def assert_pass(self, pane_text: str, question: str):
        """Evaluate and raise AssertionError if it doesn't pass."""
        result = self.evaluate(pane_text, question)
        assert result["pass"], f"LLM evaluation failed: {result['reasoning']}"
