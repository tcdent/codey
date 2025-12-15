# Codey

A terminal-based AI coding assistant built in Rust.

## Features

- **Real-time Streaming**: See responses as they're generated with live markdown rendering
- **Tool Execution**: Execute file operations, shell commands, and web fetches
- **Permission System**: All tool executions require user approval
- **Session Persistence**: Continue previous sessions with `--continue` flag
- **Multi-provider Support**: Works with any LLM provider supported by the `genai` crate

## Installation

### From Source

```bash
cargo install --path .
```

### Building

```bash
cargo build --release
```

The binary will be at `target/release/codey`.

## Usage

```bash
# Start a new session
codey

# Continue from previous session
codey --continue

# Specify a working directory
codey --working-dir /path/to/project

# Use a specific model
codey --model claude-sonnet-4-20250514

# Enable debug logging
codey --debug
```

## Configuration

Copy `config.example.toml` to `~/.config/codey/config.toml` and customize:

```toml
[general]
model = "claude-sonnet-4-20250514"
max_tokens = 8192

[ui]
theme = "base16-ocean.dark"
```

## Keybindings

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | New line in input |
| `Ctrl+C` | Quit |
| `Esc` | Clear input |
| `Up/Down` | Scroll chat (when input empty: history) |
| `PageUp/PageDown` | Page scroll |

### Tool Approval

| Key | Action |
|-----|--------|
| `y` | Allow |
| `n` | Deny |

## Tools

Codey provides five core tools:

### read_file
Read file contents with optional line ranges.

### write_file
Create new files (fails if file exists).

### edit_file
Apply search/replace edits to existing files.

### shell
Execute bash commands with optional working directory.

### fetch_url
Fetch content from URLs (HTTP/HTTPS only).

## Session Persistence

Sessions are automatically saved to `.codey/transcript.json` in the working directory. Use `codey --continue` to resume a previous session with full context restoration including tool call history.

## License

MIT
