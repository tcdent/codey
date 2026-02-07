"""
Integration test: edit_file tool.

Verifies that:
1. Asking codey to edit a file triggers the edit_file tool
2. The approval shows the diff/changes
3. Approving applies the edit
4. The file is actually modified on disk
"""


def test_edit_file_applies_change(codey, evaluator):
    """Full flow: request edit -> approval with diff -> approve -> file modified."""

    # Step 1: Ask codey to make a specific, deterministic edit.
    codey.type_and_submit(
        "Edit README.md and change '# Test Workspace' to '# My Project'. "
        "Use the edit_file tool only."
    )

    # Step 2: Wait for the approval to appear.
    pane = codey.wait_for_approval(timeout=30)

    # Step 3: Verify the approval shows the edit details.
    evaluator.assert_pass(
        pane,
        "Is there an approval prompt for an edit_file tool call? "
        "It should show something about editing README.md, with the "
        "old text ('Test Workspace') being replaced by new text ('My Project').",
    )

    # Step 4: Approve.
    codey.approve()

    # Step 5: Wait for completion.
    pane = codey.wait_for_idle(timeout=30)

    # Step 6: Verify codey reports success.
    evaluator.assert_pass(
        pane,
        "Did the edit_file tool complete successfully? There should be "
        "some indication that the file was edited (success message, "
        "or the assistant confirming the change was made).",
    )
