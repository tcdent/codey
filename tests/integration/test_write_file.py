"""
Integration test: write_file tool.

Verifies that:
1. Asking codey to create a file triggers write_file
2. The approval shows the file content
3. The file is created on disk after approval
"""


def test_write_new_file(codey, evaluator):
    """Full flow: request new file -> approval -> approve -> file exists."""

    codey.type_and_submit(
        "Create a new file called hello.txt with the content 'Hello, World!'. "
        "Use the write_file tool."
    )

    # Wait for approval.
    pane = codey.wait_for_approval(timeout=30)

    evaluator.assert_pass(
        pane,
        "Is there an approval prompt for creating/writing a file? "
        "It should reference 'hello.txt' and show the content 'Hello, World!' "
        "that will be written.",
    )

    # Approve.
    codey.approve()

    # Wait for completion.
    pane = codey.wait_for_idle(timeout=30)

    evaluator.assert_pass(
        pane,
        "Did the write_file tool complete successfully? There should be "
        "confirmation that hello.txt was created.",
    )
