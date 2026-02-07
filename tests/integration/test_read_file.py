"""
Integration test: read_file tool.

Verifies that:
1. Asking codey to read a file triggers a tool call
2. The approval dialog renders correctly
3. Approving the tool call executes the read
4. The file contents appear in the output
"""


def test_read_file_approval_and_output(codey, evaluator):
    """Full flow: prompt -> approval renders -> approve -> output appears."""

    # Step 1: Ask codey to read a known file.
    codey.type_and_submit(
        "Read the file README.md and tell me what it says. "
        "Do not use any other tools."
    )

    # Step 2: Wait for the approval to appear.
    pane = codey.wait_for_approval(timeout=30)

    # Step 3: LLM evaluates: does the approval look right?
    evaluator.assert_pass(
        pane,
        "Is there a tool approval prompt visible for reading a file "
        "(something like read_file or mcp_read_file with README.md as a parameter)? "
        "The approval should show the tool name and the file path.",
    )

    # Step 4: Approve the tool call.
    codey.approve()

    # Step 5: Wait for the tool output and response to complete.
    pane = codey.wait_for_text("Test Workspace", timeout=30)

    # Step 6: Verify the file contents appeared.
    evaluator.assert_pass(
        pane,
        "Did the tool successfully read a file and display its contents? "
        "The output should contain '# Test Workspace' or similar content "
        "from a README.md file.",
    )


def test_read_file_deny(codey, evaluator):
    """Denying a tool call should cancel it and return to input."""

    codey.type_and_submit("Read the file README.md")

    pane = codey.wait_for_approval(timeout=30)

    # Deny the tool call.
    codey.deny()

    # Should return to idle state without executing.
    pane = codey.wait_for_idle(timeout=15)

    evaluator.assert_pass(
        pane,
        "Was the tool call denied/cancelled? There should be an indication "
        "that the tool was not executed (e.g., 'denied', 'cancelled', or "
        "the assistant acknowledging the denial) and no file contents shown.",
    )
