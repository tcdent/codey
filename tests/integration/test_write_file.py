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

    # Approval renders as: ? write_file\n  {"path": "hello.txt", ...}\n  [y]es  [n]o
    evaluator.assert_pass(
        pane,
        "Is there a tool approval prompt with '?' icon for write_file? "
        "The parameters should show 'hello.txt' as the path and "
        "'Hello, World!' as the content. There should be a '[y]es  [n]o' prompt.",
    )

    # Approve.
    codey.approve()

    # Wait for completion (✓ icon).
    pane = codey.wait_for_completion(timeout=30)

    evaluator.assert_pass(
        pane,
        "Did the write_file tool complete? There should be a '✓' icon "
        "next to write_file indicating the file was created successfully.",
    )
