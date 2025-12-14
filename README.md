# Codepal

A terminal-based AI coding assistant built in Rust, powered by Claude.

## Features

- **Real-time Streaming**: See Claude's responses as they're generated
- **Tool Execution**: Execute file operations, shell commands, and web fetches
- **Permission System**: All tool executions require user approval
- **Syntax Highlighting**: Code blocks are highlighted with syntect
- **OAuth Support**: Authenticate with Anthropic OAuth for Claude Max/Pro users
- **Search/Replace Edits**: Precise file editing with exact match patterns

## Installation

### From Source

```bash
cargo install --path .
```

### Building

```bash
cargo build --release
```

The binary will be at `target/release/codepal`.

## Usage

```bash
# Start with default settings
codepal

# Specify a working directory
codepal --working-dir /path/to/project

# Use a specific model
codepal --model claude-sonnet-4-20250514

# Use API key authentication
ANTHROPIC_API_KEY=sk-ant-... codepal

# Enable debug logging
codepal --debug
```

## Configuration

Copy `config.example.toml` to `~/.config/codepal/config.toml` and customize:

```toml
[general]
model = "claude-sonnet-4-20250514"
max_tokens = 8192

[auth]
method = "oauth"  # or "api_key"
# api_key = "sk-ant-..."

[ui]
theme = "base16-ocean.dark"
auto_scroll = true
show_tokens = true
```

## Keybindings

| Key | Action |
|-----|--------|
| `Ctrl+Enter` | Send message |
| `Enter` | New line in input |
| `Ctrl+C` | Quit |
| `Ctrl+L` | Clear chat |
| `Esc` | Clear input |
| `Up/Down` | Scroll chat (when input empty: history) |
| `PageUp/PageDown` | Page scroll |

### Permission Dialog

| Key | Action |
|-----|--------|
| `y` / `Enter` | Allow |
| `n` / `Esc` | Deny |
| `a` | Allow for session |
| `Tab` / `Arrow keys` | Navigate options |

## Tools

Codepal provides five core tools:

### read_file
Read file contents with optional line ranges.
```
read_file(path, start_line?, end_line?)
```

### write_file
Create new files (fails if file exists).
```
write_file(path, content)
```

### edit_file
Apply search/replace edits to existing files.
```
edit_file(path, edits: [{old_string, new_string}])
```

### shell
Execute bash commands with optional working directory.
```
shell(command, working_dir?)
```

### fetch_url
Fetch content from URLs (HTTP/HTTPS only).
```
fetch_url(url, max_length?)
```

## Authentication

### OAuth (Recommended for Claude Max/Pro)

1. Run `codepal`
2. Visit the displayed URL
3. Enter the code shown
4. Tokens are saved to `~/.config/codepal/auth.json`

### API Key

Set the `ANTHROPIC_API_KEY` environment variable or configure in `config.toml`:

```toml
[auth]
method = "api_key"
api_key = "sk-ant-..."
```

## License

MIT
