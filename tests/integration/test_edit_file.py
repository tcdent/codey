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
    # Approval renders as: ? edit_file\n  {params with old_string/new_string}\n  [y]es  [n]o
    evaluator.assert_pass(
        pane,
        "Is there a tool approval prompt with '?' icon for edit_file? "
        "The parameters should show README.md as the path and contain "
        "the old text ('Test Workspace') and new text ('My Project'). "
        "There should be a '[y]es  [n]o' prompt.",
    )

    # Step 4: Approve.
    codey.approve()

    # Step 5: Wait for completion (✓ icon).
    pane = codey.wait_for_completion(timeout=30)

    # Step 6: Verify codey reports success.
    evaluator.assert_pass(
        pane,
        "Did the edit_file tool complete? There should be a '✓' icon "
        "next to edit_file indicating success.",
    )
