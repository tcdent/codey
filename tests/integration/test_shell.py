"""
Integration test: shell tool.

Verifies that:
1. Asking codey to run a command triggers the shell tool
2. The approval shows the command to be executed
3. The command output appears after approval
"""


def test_shell_command_execution(codey, evaluator):
    """Full flow: request command -> approval -> approve -> output shown."""

    # Step 1: Ask codey to run a simple, deterministic command.
    codey.type_and_submit(
        "Run the command: echo 'hello from codey test'. "
        "Use the shell tool."
    )

    # Step 2: Wait for approval.
    pane = codey.wait_for_approval(timeout=30)

    # Step 3: Verify the approval shows the command.
    evaluator.assert_pass(
        pane,
        "Is there an approval prompt for a shell command? "
        "It should show the command 'echo' with 'hello from codey test' "
        "or similar, waiting for user confirmation.",
    )

    # Step 4: Approve.
    codey.approve()

    # Step 5: Wait for the output to appear.
    pane = codey.wait_for_text("hello from codey test", timeout=30)

    # Step 6: Verify the command output is displayed.
    evaluator.assert_pass(
        pane,
        "Did the shell command execute and display its output? "
        "The text 'hello from codey test' should appear in the terminal "
        "as the command's output.",
    )


def test_shell_multiline_output(codey, evaluator):
    """Verify that multi-line shell output renders correctly."""

    codey.type_and_submit(
        "Run this shell command: ls -la /workspace"
    )

    pane = codey.wait_for_approval(timeout=30)
    codey.approve()

    # Wait for the listing to appear (should contain README.md and .git).
    pane = codey.wait_for_text("README.md", timeout=30)

    evaluator.assert_pass(
        pane,
        "Does the terminal show a directory listing from 'ls -la'? "
        "It should show file entries including README.md and .git, "
        "with permissions, sizes, and dates formatted in a readable way.",
    )
