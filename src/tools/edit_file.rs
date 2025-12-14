//! Edit file tool with search/replace

use super::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::path::Path;

/// Tool for editing existing files with search/replace
pub struct EditFileTool;

#[derive(Debug, Deserialize)]
struct EditFileParams {
    path: String,
    edits: Vec<SearchReplace>,
}

#[derive(Debug, Deserialize)]
struct SearchReplace {
    old_string: String,
    new_string: String,
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Apply search/replace edits to an existing file. Each old_string must match exactly \
         and appear exactly once in the file. Edits are applied sequentially. \
         Use read_file first to see the current file contents."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "edits": {
                    "type": "array",
                    "description": "List of search/replace operations to apply sequentially",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": {
                                "type": "string",
                                "description": "Exact string to find (must be unique in file)"
                            },
                            "new_string": {
                                "type": "string",
                                "description": "String to replace it with"
                            }
                        },
                        "required": ["old_string", "new_string"]
                    }
                }
            },
            "required": ["path", "edits"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let params: EditFileParams = serde_json::from_value(params)?;
        let path = Path::new(&params.path);

        // Check if file exists
        if !path.exists() {
            return Ok(ToolResult::error(format!(
                "File not found: {}. Use write_file to create new files.",
                params.path
            )));
        }

        // Check if it's a file
        if !path.is_file() {
            return Ok(ToolResult::error(format!(
                "Not a file: {}",
                params.path
            )));
        }

        // Read current content
        let mut content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult::error(format!(
                    "Failed to read file: {}",
                    e
                )));
            }
        };

        // Validate edits before applying
        for (i, edit) in params.edits.iter().enumerate() {
            if edit.old_string == edit.new_string {
                return Ok(ToolResult::error(format!(
                    "Edit {}: old_string and new_string are identical",
                    i + 1
                )));
            }

            if edit.old_string.is_empty() {
                return Ok(ToolResult::error(format!(
                    "Edit {}: old_string cannot be empty",
                    i + 1
                )));
            }
        }

        // Apply edits sequentially
        let mut applied_edits = Vec::new();

        for (i, edit) in params.edits.iter().enumerate() {
            let matches: Vec<_> = content.match_indices(&edit.old_string).collect();

            match matches.len() {
                0 => {
                    // Provide helpful context for debugging
                    let preview = if edit.old_string.len() > 50 {
                        format!("{}...", &edit.old_string[..50])
                    } else {
                        edit.old_string.clone()
                    };
                    return Ok(ToolResult::error(format!(
                        "Edit {}: old_string not found in file.\n\nSearching for:\n{}\n\n\
                         Tip: Make sure the string matches exactly, including whitespace and indentation.",
                        i + 1,
                        preview
                    )));
                }
                1 => {
                    // Unique match - apply the edit
                    content = content.replacen(&edit.old_string, &edit.new_string, 1);
                    applied_edits.push(format!(
                        "Edit {}: Replaced {} chars with {} chars",
                        i + 1,
                        edit.old_string.len(),
                        edit.new_string.len()
                    ));
                }
                n => {
                    return Ok(ToolResult::error(format!(
                        "Edit {}: old_string found {} times (must be unique). \
                         Include more surrounding context to make the match unique.",
                        i + 1,
                        n
                    )));
                }
            }
        }

        // Write the modified content
        match fs::write(path, &content) {
            Ok(()) => {
                let summary = applied_edits.join("\n");
                Ok(ToolResult::success(format!(
                    "Successfully applied {} edit(s) to {}\n\n{}",
                    params.edits.len(),
                    params.path,
                    summary
                )))
            }
            Err(e) => Ok(ToolResult::error(format!("Failed to write file: {}", e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_edit_file_single() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "edits": [
                    {
                        "old_string": "println!(\"hello\")",
                        "new_string": "println!(\"hello, world!\")"
                    }
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error, "Error: {}", result.content);

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("hello, world!"));
    }

    #[tokio::test]
    async fn test_edit_file_multiple() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(
            &file_path,
            "fn foo() {}\n\nfn bar() {}\n\nfn baz() {}",
        )
        .unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "edits": [
                    {
                        "old_string": "fn foo() {}",
                        "new_string": "fn foo() -> i32 { 1 }"
                    },
                    {
                        "old_string": "fn bar() {}",
                        "new_string": "fn bar() -> i32 { 2 }"
                    }
                ]
            }))
            .await
            .unwrap();

        assert!(!result.is_error);

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("fn foo() -> i32 { 1 }"));
        assert!(content.contains("fn bar() -> i32 { 2 }"));
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tool = EditFileTool;
        let result = tool
            .execute(json!({
                "path": "/nonexistent/file.rs",
                "edits": [
                    {
                        "old_string": "foo",
                        "new_string": "bar"
                    }
                ]
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_edit_file_ambiguous() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "foo foo foo").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "edits": [
                    {
                        "old_string": "foo",
                        "new_string": "bar"
                    }
                ]
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("3 times"));
    }

    #[tokio::test]
    async fn test_edit_file_string_not_found() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "hello world").unwrap();

        let tool = EditFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "edits": [
                    {
                        "old_string": "goodbye",
                        "new_string": "farewell"
                    }
                ]
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }
}
