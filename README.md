# Codey

A terminal-based AI coding assistant built in Rust.

<img width="905" height="1143" alt="Screenshot 2026-01-06 at 4 10 53 PM" src="https://github.com/user-attachments/assets/e23f2009-1e64-4068-8a86-991a655bae10" />

## Features

- **Real-time Streaming**: See responses as they're generated with live markdown rendering
- **Tool Execution**: Bash for everything on the system, plus web fetch and search
- **Sub-agents**: Spawn background agents for research tasks
- **Session Persistence**: Continue previous sessions with full context restoration
- **Subscription or API billing**: runs on the Claude Agent SDK via the `claude` binary (subscription login or `ANTHROPIC_API_KEY`)

## Installation

### Homebrew (macOS/Linux)

```bash
brew install tcdent/tap/codey
```

### Download Binary

Download the latest release for your platform from the [GitHub Releases](https://github.com/tcdent/codey/releases/latest):

- **macOS (Apple Silicon)**: `codey-darwin-arm64.tar.gz`
- **Linux (x86_64)**: `codey-linux-x86_64.tar.gz`
- **Linux (ARM64)**: `codey-linux-arm64.tar.gz`

```bash
# Example for macOS ARM
tar -xzf codey-darwin-arm64.tar.gz
sudo mv codey-darwin-arm64 /usr/local/bin/codey
```

### From Source

```bash
git clone https://github.com/tcdent/codey.git
cd codey
make release
sudo cp target/release/codey /usr/local/bin/
```

### Requirements

- **Claude CLI** (required): Codey drives the `claude` binary via the Claude Agent SDK. Install it from https://docs.claude.com/claude-code and make sure `claude` is on your `PATH`.
- **Authentication**: handled by the `claude` binary — log in once with `claude` (subscription) or set `ANTHROPIC_API_KEY` for pay-as-you-go.

## Usage

```bash
# Start a new session
codey

# Continue from previous session
codey --continue

# Specify a working directory
codey --working-dir /path/to/project
```

### Authentication

Codey itself stores no credentials — the `claude` binary it drives owns auth.
Codey runs the binary with `CLAUDE_CONFIG_DIR=~/.codey/claude` so its settings,
sessions, and credentials stay isolated from your interactive `~/.claude`.
Subscription logins live in that dir, so the one-time login must be run with it
set; API keys come from the environment and need no login:

```bash
# Subscription login (one time, stored under ~/.codey/claude)
CLAUDE_CONFIG_DIR=~/.codey/claude claude   # then /login

# Or use an API key (pay-as-you-go)
export ANTHROPIC_API_KEY="sk-ant-..."
```

## Configuration

Copy `config.example.toml` to `~/.config/codey/config.toml` and customize:

```toml
[agents.foreground]
model = "claude-opus-4-6"
max_tokens = 8192

[agents.background]
model = "claude-sonnet-4-5-20250929"

[ui]
theme = "base16-ocean.dark"
```

Foreground and background agents are configured independently. Both default to `claude-opus-4-6` when not specified. See `config.example.toml` for all available options.

## Agent Persona

Customize the agent's name and personality:

```toml
[agent]
name = "Jarvis"
system_prompt = "You are Jarvis, a sophisticated AI assistant."
```

The `name` appears in the chat header and welcome message (default: "Codey"). The `system_prompt` replaces the default intro paragraph while keeping the built-in capabilities and guidelines.

## Custom System Prompts

You can extend Codey's system prompt by creating `SYSTEM.md` files that are automatically appended to the base prompt. These files are loaded from two locations (in order):

1. **User config**: `~/.config/codey/SYSTEM.md` - personal customizations
2. **Project**: `.codey/SYSTEM.md` - project-specific instructions

### Dynamic Content with esh

SYSTEM.md files support [esh](https://github.com/jirutka/esh) (Embedded SHell) syntax, allowing you to embed shell commands that are executed dynamically. This is useful for including context that changes over time.

### Example SYSTEM.md

```markdown
## Environment

- Today's date: <%= $(date +"%m-%d-%Y") %>
- Project root: <%= $(pwd) %>

<% if which linctl > /dev/null 2>&1; then -%>
Use linctl to manage Linear tickets.
<% fi -%>

## Guidelines
- Follow the existing code style
```

Commands are re-executed on every LLM request, so the prompt always reflects the current state of your environment. Note that this may cause cache invalidation if command output changes between requests.

## Keybindings

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | New line in input |
| `Esc` / `Ctrl+C` | Cancel (layered, see below) |
| `Up/Down` | Scroll chat (when input empty: history) |
| `PageUp/PageDown` | Page scroll |

### Cancellation

Esc and Ctrl+C use layered cancellation that stacks based on what's happening:

1. **Running foreground tool** → cancels the tool (kills the process), error is sent to the agent which continues its turn
2. **Agent streaming** → ends the turn entirely
3. **Idle** → clears the input

Background tasks are never affected by cancel.

## Tools

Codey provides five tools, and runs them without asking: allowing and denying
belongs to the shell environment the agent runs in.

| Tool | Description |
|------|-------------|
| `shell` | Execute bash commands with optional working directory; reading, searching and editing files all go through it |
| `fetch_url` | Fetch content from URLs (HTTP/HTTPS) |
| `web_search` | Search the web and return results |
| `spawn_agent` | Spawn a sub-agent for research/analysis tasks; its result arrives as a notification |
| `list_agents` | List all sub-agents and their status |

## Session Persistence

Sessions are saved to `.codey/transcripts/` in the working directory. Use `codey --continue` to resume with full context restoration.

## License

MIT
