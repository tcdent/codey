//! Write file tool

use super::{Tool, ToolResult};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::path::Path;

/// Tool for creating new files
pub struct WriteFileTool;

#[derive(Debug, Deserialize)]
struct WriteFileParams {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create a new file with the specified content. Fails if the file already exists. \
         Use edit_file to modify existing files."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path where the file will be created"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult> {
        let params: WriteFileParams = serde_json::from_value(params)?;
        let path = Path::new(&params.path);

        // Check if file already exists
        if path.exists() {
            return Ok(ToolResult::error(format!(
                "File already exists: {}. Use edit_file to modify existing files.",
                params.path
            )));
        }

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return Ok(ToolResult::error(format!(
                        "Failed to create parent directories: {}",
                        e
                    )));
                }
            }
        }

        // Write file
        match fs::write(path, &params.content) {
            Ok(()) => {
                let line_count = params.content.lines().count();
                let byte_count = params.content.len();
                Ok(ToolResult::success(format!(
                    "Created file: {} ({} lines, {} bytes)",
                    params.path, line_count, byte_count
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
    async fn test_write_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("new_file.txt");

        let tool = WriteFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "Hello, World!\nLine 2"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Created file"));

        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, World!\nLine 2");
    }

    #[tokio::test]
    async fn test_write_existing_file_fails() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(&file_path, "existing content").unwrap();

        let tool = WriteFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "new content"
            }))
            .await
            .unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("already exists"));
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("subdir").join("nested").join("file.txt");

        let tool = WriteFileTool;
        let result = tool
            .execute(json!({
                "path": file_path.to_str().unwrap(),
                "content": "nested content"
            }))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(file_path.exists());
    }
}
