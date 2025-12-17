//! Slash command system for app control

use anyhow::Result;

/// Available slash commands
#[derive(Debug, Clone)]
pub enum Command {
    Compact,
}

/// Parse input text and return command if it starts with /
pub fn parse_command(input: &str) -> Result<Command> {
    let input = input.trim();
    
    if !input.starts_with('/') {
        anyhow::bail!("Not a command");
    }
    
    let cmd_name = input[1..].split_whitespace().next()
        .ok_or_else(|| anyhow::anyhow!("Empty command"))?;
    
    match cmd_name {
        "compact" => Ok(Command::Compact),
        unknown => anyhow::bail!("Unknown command: /{}", unknown),
    }
}

impl Command {
    /// Execute the command with access to app and agent
    pub fn execute(&self, app: &mut crate::app::App, agent: &mut crate::llm::Agent) -> Result<()> {
        match self {
            Command::Compact => {
                // Queue compaction through normal message system
                app.queue_compaction();
                Ok(())
            }
        }
    }
    
    /// Get the display text for this command
    pub fn display_text(&self) -> &'static str {
        match self {
            Command::Compact => "/compact",
        }
    }
}
