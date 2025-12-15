//! Permission handling for tool execution

/// Decision for a single tool execution request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDecision {
    Approve,
    Deny,
}
