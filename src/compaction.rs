//! Context compaction for managing token usage
//!
//! When the conversation context exceeds a threshold, this module handles
//! asking the agent to summarize the conversation for continuation in a
//! new transcript.

use serde::{Deserialize, Serialize};
use serde_json::json;

/// The prompt sent to the agent to generate a compaction summary
pub const COMPACTION_PROMPT: &str = r#"The conversation context is getting large and needs to be compacted.

Please provide a comprehensive summary of our conversation so far. Your response must be valid JSON matching the required schema.

Be thorough but concise - this summary will seed a fresh conversation context."#;

/// Represents a compaction summary from the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSummary {
    /// List of main tasks and changes completed during this conversation
    pub accomplished: Vec<String>,
    /// Remaining tasks, open questions, or next steps not yet completed
    pub remaining: Vec<String>,
    /// Important facts about the project (architecture, patterns, gotchas)
    pub project_info: Vec<String>,
    /// Files most relevant to the work being done
    pub relevant_files: Vec<RelevantFile>,
}

/// A file that was relevant to the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevantFile {
    /// File path relative to project root
    pub path: String,
    /// Brief description of why this file is relevant
    pub description: String,
}

impl CompactionSummary {
    /// Generate JSON schema for structured output
    pub fn json_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "accomplished": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of main tasks and changes completed during this conversation"
                },
                "remaining": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Remaining tasks, open questions, or next steps not yet completed"
                },
                "project_info": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Important facts about the project (architecture decisions, coding patterns, gotchas)"
                },
                "relevant_files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "File path relative to project root"
                            },
                            "description": {
                                "type": "string",
                                "description": "Brief description of why this file is relevant"
                            }
                        },
                        "required": ["path", "description"]
                    },
                    "description": "Files most relevant to the work being done"
                }
            },
            "required": ["accomplished", "remaining", "project_info", "relevant_files"]
        })
    }

    /// Format the summary for display and for seeding a new context
    pub fn format_for_context(&self) -> String {
        let mut output = String::from("# Previous Session Summary\n\n");
        output.push_str("The following is a summary from a previous conversation that was compacted due to context length.\n\n");

        if !self.accomplished.is_empty() {
            output.push_str("## What Was Accomplished\n");
            for item in &self.accomplished {
                output.push_str(&format!("- {}\n", item));
            }
            output.push('\n');
        }

        if !self.remaining.is_empty() {
            output.push_str("## What Still Needs To Be Done\n");
            for item in &self.remaining {
                output.push_str(&format!("- {}\n", item));
            }
            output.push('\n');
        }

        if !self.project_info.is_empty() {
            output.push_str("## Key Project Information\n");
            for item in &self.project_info {
                output.push_str(&format!("- {}\n", item));
            }
            output.push('\n');
        }

        if !self.relevant_files.is_empty() {
            output.push_str("## Relevant Files\n");
            for file in &self.relevant_files {
                output.push_str(&format!("- `{}` - {}\n", file.path, file.description));
            }
            output.push('\n');
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_schema_generation() {
        let schema = CompactionSummary::json_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["accomplished"].is_object());
        assert!(schema["properties"]["relevant_files"].is_object());
    }

    #[test]
    fn test_format_for_context() {
        let summary = CompactionSummary {
            accomplished: vec!["Fixed authentication bug".to_string()],
            remaining: vec!["Add unit tests".to_string()],
            project_info: vec!["Uses JWT for auth".to_string()],
            relevant_files: vec![RelevantFile {
                path: "src/auth.rs".to_string(),
                description: "Authentication module".to_string(),
            }],
        };
        let formatted = summary.format_for_context();
        assert!(formatted.contains("Previous Session Summary"));
        assert!(formatted.contains("Fixed authentication bug"));
        assert!(formatted.contains("Add unit tests"));
        assert!(formatted.contains("src/auth.rs"));
    }

    #[test]
    fn test_deserialize_summary() {
        let json = r#"{
            "accomplished": ["Task 1", "Task 2"],
            "remaining": ["Task 3"],
            "project_info": ["Info 1"],
            "relevant_files": [{"path": "src/main.rs", "description": "Entry point"}]
        }"#;
        let summary: CompactionSummary = serde_json::from_str(json).unwrap();
        assert_eq!(summary.accomplished.len(), 2);
        assert_eq!(summary.remaining.len(), 1);
    }
}
