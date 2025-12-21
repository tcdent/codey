//! Edit file tool with search/replace

use super::{once_ready, Tool, ToolEffect, ToolOutput, ToolResult};
use crate::ide::ToolPreview;
use crate::impl_base_block;
use crate::transcript::{render_approval_prompt, render_result, Block, BlockType, ToolBlock, Status};
use futures::stream::BoxStream;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// Edit file display block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditFileBlock {
    pub call_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub status: Status,
    pub text: String,
}

impl EditFileBlock {
    pub fn new(call_id: impl Into<String>, tool_name: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            params,
            status: Status::Pending,
            text: String::new(),
        }
    }

    pub fn from_params(call_id: &str, tool_name: &str, params: serde_json::Value) -> Option<Self> {
        let _: EditFileParams = serde_json::from_value(params.clone()).ok()?;
        Some(Self::new(call_id, tool_name, params))
    }
}

#[typetag::serde]
impl Block for EditFileBlock {
    impl_base_block!(BlockType::Tool);

    fn render(&self, _width: u16) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let path = self.params["path"].as_str().unwrap_or("");
        let edit_count = self.params.get("edits").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

        // Format: edit_file(path, N edits)
        lines.push(Line::from(vec![
            self.render_status(),
            Span::styled("edit_file", Style::default().fg(Color::Magenta)),
            Span::styled("(", Style::default().fg(Color::DarkGray)),
            Span::styled(path, Style::default().fg(Color::Yellow)),
            Span::styled(format!(", {} edit{}", edit_count, if edit_count == 1 { "" } else { "s" }), Style::default().fg(Color::DarkGray)),
            Span::styled(")", Style::default().fg(Color::DarkGray)),
        ]));

        if self.status == Status::Pending {
            lines.push(render_approval_prompt());
        }

        if !self.text.is_empty() {
            lines.extend(render_result(&self.text, 5));
        }

        if self.status == Status::Denied {
            lines.push(Line::from(Span::styled(
                "  Denied by user",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn call_id(&self) -> Option<&str> {
        Some(&self.call_id)
    }

    fn tool_name(&self) -> Option<&str> {
        Some(&self.tool_name)
    }

    fn params(&self) -> Option<&serde_json::Value> {
        Some(&self.params)
    }
}

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

    fn create_block(&self, call_id: &str, params: serde_json::Value) -> Box<dyn Block> {
        if let Some(block) = EditFileBlock::from_params(call_id, self.name(), params.clone()) {
            Box::new(block)
        } else {
            Box::new(ToolBlock::new(call_id, self.name(), params))
        }
    }

    fn execute(&self, params: serde_json::Value) -> BoxStream<'static, ToolOutput> {
        once_ready(Ok(Self::execute_inner(params)))
    }

    fn ide_preview(&self, params: &serde_json::Value) -> Option<ToolPreview> {
        let file_path = params.get("path").and_then(|p| p.as_str())?;
        let edits = params.get("edits").and_then(|e| e.as_array())?;

        if edits.is_empty() {
            return None;
        }

        // Read original file content
        let original = fs::read_to_string(file_path).ok()?;

        // Apply all edits to get the modified version
        let modified = Self::apply_edits(&original, edits)?;

        Some(ToolPreview::Diff {
            path: file_path.to_string(),
            original,
            modified,
        })
    }
}

impl EditFileTool {
    fn execute_inner(params: serde_json::Value) -> ToolResult {
        let params: EditFileParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("Invalid params: {}", e)),
        };
        let path = Path::new(&params.path);

        // Check if file exists
        if !path.exists() {
            return ToolResult::error(format!(
                "File not found: {}. Use write_file to create new files.",
                params.path
            ));
        }

        // Check if it's a file
        if !path.is_file() {
            return ToolResult::error(format!("Not a file: {}", params.path));
        }

        // Read current content
        let mut content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => return ToolResult::error(format!("Failed to read file: {}", e)),
        };

        // Validate edits before applying
        for (i, edit) in params.edits.iter().enumerate() {
            if edit.old_string == edit.new_string {
                return ToolResult::error(format!(
                    "Edit {}: old_string and new_string are identical",
                    i + 1
                ));
            }

            if edit.old_string.is_empty() {
                return ToolResult::error(format!("Edit {}: old_string cannot be empty", i + 1));
            }
        }

        // Apply edits sequentially
        let mut applied_edits = Vec::new();

        for (i, edit) in params.edits.iter().enumerate() {
            let matches: Vec<_> = content.match_indices(&edit.old_string).collect();

            match matches.len() {
                0 => {
                    let preview = if edit.old_string.len() > 50 {
                        format!("{}...", &edit.old_string[..50])
                    } else {
                        edit.old_string.clone()
                    };
                    return ToolResult::error(format!(
                        "Edit {}: old_string not found in file.\n\nSearching for:\n{}\n\n\
                         Tip: Make sure the string matches exactly, including whitespace and indentation.",
                        i + 1,
                        preview
                    ));
                }
                1 => {
                    content = content.replacen(&edit.old_string, &edit.new_string, 1);
                    applied_edits.push(format!(
                        "Edit {}: Replaced {} chars with {} chars",
                        i + 1,
                        edit.old_string.len(),
                        edit.new_string.len()
                    ));
                }
                n => {
                    return ToolResult::error(format!(
                        "Edit {}: old_string found {} times (must be unique). \
                         Include more surrounding context to make the match unique.",
                        i + 1,
                        n
                    ));
                }
            }
        }

        // Write the modified content
        match fs::write(path, &content) {
            Ok(()) => {
                let summary = applied_edits.join("\n");
                let abs_path = path.canonicalize().unwrap_or_else(|_| PathBuf::from(&params.path));
                ToolResult::success(format!(
                    "Successfully applied {} edit(s) to {}\n\n{}",
                    params.edits.len(),
                    params.path,
                    summary
                )).with_effects(vec![ToolEffect::IdeReloadBuffer { path: abs_path }])
            }
            Err(e) => ToolResult::error(format!("Failed to write file: {}", e)),
        }
    }

    /// Apply edits to content, returning the modified version
    /// Returns None if any edit fails (not found or ambiguous)
    fn apply_edits(content: &str, edits: &[serde_json::Value]) -> Option<String> {
        let mut result = content.to_string();
        
        for edit in edits {
            let old_str = edit.get("old_string").and_then(|s| s.as_str())?;
            let new_str = edit.get("new_string").and_then(|s| s.as_str())?;
            
            // Check for exactly one match
            let matches: Vec<_> = result.match_indices(old_str).collect();
            if matches.len() != 1 {
                return None; // Not found or ambiguous
            }
            
            result = result.replacen(old_str, new_str, 1);
        }
        
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use tempfile::tempdir;

    async fn run_tool(tool: &EditFileTool, params: serde_json::Value) -> ToolResult {
        let mut stream = tool.execute(params);
        while let Some(output) = stream.next().await {
            if let ToolOutput::Done(r) = output {
                return r;
            }
        }
        panic!("Tool should return Done");
    }

    #[tokio::test]
    async fn test_edit_file_single() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}").unwrap();

        let tool = EditFileTool;
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap(),
            "edits": [
                {
                    "old_string": "println!(\"hello\")",
                    "new_string": "println!(\"hello, world!\")"
                }
            ]
        })).await;

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
        let result = run_tool(&tool, json!({
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
        })).await;

        assert!(!result.is_error);

        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("fn foo() -> i32 { 1 }"));
        assert!(content.contains("fn bar() -> i32 { 2 }"));
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let tool = EditFileTool;
        let result = run_tool(&tool, json!({
            "path": "/nonexistent/file.rs",
            "edits": [
                {
                    "old_string": "foo",
                    "new_string": "bar"
                }
            ]
        })).await;

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_edit_file_ambiguous() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "foo foo foo").unwrap();

        let tool = EditFileTool;
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap(),
            "edits": [
                {
                    "old_string": "foo",
                    "new_string": "bar"
                }
            ]
        })).await;

        assert!(result.is_error);
        assert!(result.content.contains("3 times"));
    }

    #[tokio::test]
    async fn test_edit_file_string_not_found() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        fs::write(&file_path, "hello world").unwrap();

        let tool = EditFileTool;
        let result = run_tool(&tool, json!({
            "path": file_path.to_str().unwrap(),
            "edits": [
                {
                    "old_string": "goodbye",
                    "new_string": "farewell"
                }
            ]
        })).await;

        assert!(result.is_error);
        assert!(result.content.contains("not found"));
    }
}
