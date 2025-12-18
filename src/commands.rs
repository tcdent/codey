use anyhow::Result;


const ALL_COMMANDS: &[&dyn CommandImpl] = &[
    &Help,
    &Compact,
];

pub struct Command;

impl Command {
    /// Parse input and return matching command, or None
    pub fn parse(input: &str) -> Option<&'static dyn CommandImpl> {
        let input = input.trim();
        if !input.starts_with('/') {
            return None;
        }

        let cmd_name = input[1..].split_whitespace().next()?;
        ALL_COMMANDS.iter().find(|cmd| cmd.name() == cmd_name).copied()
    }

    /// Get a command by name
    pub fn get(name: &str) -> Option<&'static dyn CommandImpl> {
        ALL_COMMANDS.iter().find(|cmd| cmd.name() == name).copied()
    }

    /// Get completion for partial input, returns full command if unique match
    pub fn complete(input: &str) -> Option<String> {
        let input = input.trim();
        if !input.starts_with('/') {
            return None;
        }

        let partial = &input[1..];
        let matches: Vec<_> = ALL_COMMANDS
            .iter()
            .filter(|cmd| cmd.name().starts_with(partial))
            .collect();

        if matches.len() == 1 {
            Some(format!("/{}", matches[0].name()))
        } else {
            None
        }
    }
}


pub trait CommandImpl: Send + Sync {
    /// Command name (without the leading /)
    fn name(&self) -> &'static str;

    /// Short description for help
    fn description(&self) -> &'static str;

    /// Execute the command, optionally returning text to display
    fn execute(&self, app: &mut crate::app::App, agent: &mut crate::llm::Agent) -> Result<Option<String>>;
}


pub struct Help;

impl CommandImpl for Help {
    fn name(&self) -> &'static str {
        "help"
    }

    fn description(&self) -> &'static str {
        "Show available commands"
    }

    fn execute(&self, _app: &mut crate::app::App, _agent: &mut crate::llm::Agent) -> Result<Option<String>> {
        let mut help_text = String::from("Available commands:");
        for cmd in ALL_COMMANDS {
            help_text.push_str(&format!("\n  /{} - {}", cmd.name(), cmd.description()));
        }
        Ok(Some(help_text))
    }
}


pub struct Compact;

impl CommandImpl for Compact {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn description(&self) -> &'static str {
        "Compact conversation history to reduce context size"
    }

    fn execute(&self, app: &mut crate::app::App, _agent: &mut crate::llm::Agent) -> Result<Option<String>> {
        app.queue_compaction();
        Ok(None)
    }
}
