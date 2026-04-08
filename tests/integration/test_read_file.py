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
    # Approval renders as: ? read_file\n  {"path": "README.md"}\n  [y]es  [n]o
    evaluator.assert_pass(
        pane,
        "Is there a tool approval prompt visible? It should show a '?' status icon "
        "followed by 'read_file', with 'README.md' in the parameters below, "
        "and a '[y]es  [n]o' prompt at the bottom of the tool block.",
    )

    # Step 4: Approve the tool call.
    codey.approve()

    # Step 5: Wait for the tool to complete (✓ icon appears).
    pane = codey.wait_for_completion(timeout=30)

    # Step 6: Verify the file contents appeared.
    evaluator.assert_pass(
        pane,
        "Did the read_file tool complete? There should be a '✓' icon next to "
        "read_file, and the output should show the contents of README.md "
        "(which contains '# Test Workspace').",
    )


def test_read_file_deny(codey, evaluator):
    """Denying a tool call should cancel it and return to input."""

    codey.type_and_submit("Read the file README.md")

    pane = codey.wait_for_approval(timeout=30)

    # Deny the tool call.
    codey.deny()

    # Should show the denial indicator (⊘ icon + "Denied by user").
    pane = codey.wait_for_denial(timeout=15)

    evaluator.assert_pass(
        pane,
        "Was the tool call denied? There should be a '⊘' icon next to "
        "read_file and the text 'Denied by user' below it. "
        "No file contents should be shown.",
    )
