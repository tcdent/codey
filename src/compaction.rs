//! Context compaction for managing token usage
//!
//! When the conversation context exceeds a threshold, this module handles
//! asking the agent to summarize the conversation for continuation in a
//! new transcript.

use serde::{Deserialize, Serialize};

/// The prompt sent to the agent to generate a compaction summary
pub const COMPACTION_PROMPT: &str = r#"The conversation context is getting large. Please provide a comprehensive summary to continue in a fresh context.

Respond with a summary in the following format:

## What Was Accomplished
List the main tasks and changes that were completed during this conversation.

## What Still Needs To Be Done
List any remaining tasks, open questions, or next steps that were discussed but not completed.

## Key Project Information
Share important facts about the project that would help a developer continuing this work:
- Architecture decisions
- Coding patterns or conventions used
- Important dependencies or integrations
- Any gotchas or things to watch out for

## Relevant Files
List the files that were most relevant to the work being done, with a brief note about each:
- path/to/file.rs - description of relevance

Be thorough but concise. This summary will be used to seed a new conversation context."#;

/// Represents a compaction summary from the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionSummary {
    /// What was accomplished during the conversation
    pub accomplished: String,
    /// What still needs to be done
    pub remaining: String,
    /// Key information about the project
    pub project_info: String,
    /// Relevant files with descriptions
    pub relevant_files: Vec<RelevantFile>,
    /// The raw summary text from the agent
    pub raw_summary: String,
}

/// A file that was relevant to the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevantFile {
    pub path: String,
    pub description: String,
}

impl CompactionSummary {
    /// Create a new compaction summary from the raw agent response
    ///
    /// For now, we just store the raw summary. In the future, we could
    /// parse the structured sections.
    pub fn from_raw(summary: String) -> Self {
        Self {
            accomplished: String::new(),
            remaining: String::new(),
            project_info: String::new(),
            relevant_files: Vec::new(),
            raw_summary: summary,
        }
    }

    /// Format the summary for display and for seeding a new context
    pub fn format_for_context(&self) -> String {
        format!(
            "# Previous Session Summary\n\n\
             The following is a summary from a previous conversation that was compacted \
             due to context length. Use this information to continue the work.\n\n\
             {}\n",
            self.raw_summary
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_summary_from_raw() {
        let raw = "## What Was Accomplished\n- Fixed bug\n\n## What Still Needs To Be Done\n- Tests".to_string();
        let summary = CompactionSummary::from_raw(raw.clone());
        assert_eq!(summary.raw_summary, raw);
    }

    #[test]
    fn test_format_for_context() {
        let summary = CompactionSummary::from_raw("Test summary".to_string());
        let formatted = summary.format_for_context();
        assert!(formatted.contains("Previous Session Summary"));
        assert!(formatted.contains("Test summary"));
    }
}
