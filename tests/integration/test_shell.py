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
    # Approval renders as: ? shell\n  {"command": "echo ..."}\n  [y]es  [n]o
    evaluator.assert_pass(
        pane,
        "Is there a tool approval prompt with '?' icon for shell? "
        "The parameters should show a command containing 'echo' and "
        "'hello from codey test'. There should be a '[y]es  [n]o' prompt.",
    )

    # Step 4: Approve.
    codey.approve()

    # Step 5: Wait for completion with the output text.
    pane = codey.wait_for_text("hello from codey test", timeout=30)

    # Step 6: Verify the tool completed with output.
    evaluator.assert_pass(
        pane,
        "Did the shell tool complete (✓ icon) and display its output? "
        "The text 'hello from codey test' should appear as the command output "
        "below the tool block header.",
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
