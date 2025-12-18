//! Slash command system for app control

use anyhow::Result;

/// Trait for slash commands
pub trait Command: Send + Sync {
    /// Command name (without the leading /)
    fn name(&self) -> &'static str;
    
    /// Short description for help
    fn description(&self) -> &'static str;
    
    /// Execute the command
    fn execute(&self, app: &mut crate::app::App, agent: &mut crate::llm::Agent) -> Result<()>;
}

// ============================================================================
// Built-in Commands
// ============================================================================

/// Compact conversation history
pub struct Compact;

impl Command for Compact {
    fn name(&self) -> &'static str {
        "compact"
    }
    
    fn description(&self) -> &'static str {
        "Compact conversation history to reduce context size"
    }
    
    fn execute(&self, app: &mut crate::app::App, _agent: &mut crate::llm::Agent) -> Result<()> {
        app.queue_compaction();
        Ok(())
    }
}

// ============================================================================
// Command Registry (static)
// ============================================================================

/// All available commands
const COMMANDS: &[&dyn Command] = &[&Compact];

/// Get a command by name
pub fn get(name: &str) -> Option<&'static dyn Command> {
    COMMANDS.iter().find(|cmd| cmd.name() == name).copied()
}

/// Get completion for partial input, returns full command if unique match
pub fn complete(input: &str) -> Option<String> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    
    let partial = &input[1..];
    let matches: Vec<_> = COMMANDS
        .iter()
        .filter(|cmd| cmd.name().starts_with(partial))
        .collect();
    
    if matches.len() == 1 {
        Some(format!("/{}", matches[0].name()))
    } else {
        None
    }
}

/// Parse input and return matching command, or None
pub fn parse(input: &str) -> Option<&'static dyn Command> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    
    let cmd_name = input[1..].split_whitespace().next()?;
    
    COMMANDS
        .iter()
        .find(|cmd| cmd.name() == cmd_name)
        .copied()
}
