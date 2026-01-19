# Codey

A terminal-based AI coding assistant built in Rust.

<img width="905" height="1143" alt="Screenshot 2026-01-06 at 4 10 53 PM" src="https://github.com/user-attachments/assets/e23f2009-1e64-4068-8a86-991a655bae10" />

## Features

- **Real-time Streaming**: See responses as they're generated with live markdown rendering
- **Tool Execution**: File operations, shell commands, web search, and more
- **Permission System**: Tool executions require approval with configurable auto-approve/deny patterns
- **IDE Integration**: Neovim integration for diff previews and buffer management
- **Sub-agents**: Spawn background agents for research tasks
- **Session Persistence**: Continue previous sessions with full context restoration
- **OAuth Authentication**: Secure authentication via Anthropic OAuth

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

- **Neovim** (optional, for IDE integration): `brew install neovim`
- **Authentication**: Either OAuth (`codey --login`) or `ANTHROPIC_API_KEY` environment variable

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

```bash
# OAuth login (recommended)
codey --login              # Prints auth URL
codey --login <code>       # Exchanges code for token

# Or use API key
export ANTHROPIC_API_KEY="sk-ant-..."
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

Codey provides eleven tools:

| Tool | Description |
|------|-------------|
| `read_file` | Read file contents with optional line ranges |
| `write_file` | Create new files (fails if file exists) |
| `edit_file` | Apply search/replace edits to existing files |
| `shell` | Execute bash commands with optional working directory |
| `fetch_url` | Fetch content from URLs (HTTP/HTTPS) |
| `fetch_html` | Fetch web pages as readable markdown using headless browser |
| `web_search` | Search the web and return results |
| `open_file` | Open a file in the IDE at a specific line |
| `spawn_agent` | Spawn a sub-agent for research/analysis tasks |
| `list_background_tasks` | List all background tasks and their status |
| `get_background_task` | Retrieve the result of a completed background task |

### Tool Filters

Configure auto-approve and auto-deny patterns in `config.toml`:

```toml
[tools.shell]
allow = ["^ls\\b", "^grep\\b"]  # Auto-approve
deny = ["rm\\s+-rf\\s+/"]       # Auto-deny (blocked)
```

Evaluation order: deny patterns → allow patterns → prompt user.

## Neovim Integration

Codey integrates with Neovim to provide real-time previews, buffer synchronization, and seamless navigation. This requires launching Neovim with an RPC socket.

### Socket Setup

Start Neovim with a listening socket:

```bash
# With tmux (recommended) - socket auto-discovered by Codey
nvim --listen /tmp/nvim-$(tmux display -p '#S').sock

# Without tmux - set the environment variable
export NVIM_LISTEN_ADDRESS=/tmp/nvim.sock
nvim --listen $NVIM_LISTEN_ADDRESS
```

### Socket Discovery

Codey discovers the Neovim socket in this order:

1. **Explicit config**: `socket` path in `config.toml`
2. **Tmux auto-discovery**: `/tmp/nvim-{session-name}.sock`
3. **Environment variable**: `$NVIM_LISTEN_ADDRESS`

### IDE Effect Handlers

When connected, Codey provides these handlers that integrate with the built-in tools:

| Handler | Tool Integration | Description |
|---------|------------------|-------------|
| **Diff Preview** | `edit_file` | Opens side-by-side diff view showing original vs. modified content before you approve changes |
| **File Preview** | `write_file` | Shows new file content in a scratch buffer before creation |
| **Buffer Reload** | `edit_file`, `write_file` | Automatically reloads open buffers after files are modified |
| **Navigation** | `open_file` | Jumps to specific file:line:column in the editor |
| **Selection Context** | Input | Visual selections in Neovim are automatically attached as context for your next prompt |
| **Unsaved Check** | `edit_file` | Prevents edits to files with unsaved changes in the buffer |

### Preview Controls

When a preview is displayed:
- Press `q` to close the preview and return to your original buffer
- Diff views show deletions (left) and additions (right) with syntax highlighting

### Configuration

Add to `~/.config/codey/config.toml`:

```toml
[ide.nvim]
enabled = true                          # Enable nvim integration (default: true)
socket = "/tmp/nvim-custom.sock"        # Explicit socket path (optional)
show_diffs = true                       # Show diff previews (default: true)
auto_reload = true                      # Auto-reload buffers (default: true)
```

## Browser Setup (for fetch_html)

The `fetch_html` tool requires Chrome or Chromium:

**macOS:**
```bash
brew install --cask chromium
# or install Google Chrome from https://google.com/chrome
```

**Debian/Ubuntu:**
```bash
sudo apt install chromium-browser
```

## Session Persistence

Sessions are saved to `.codey/transcripts/` in the working directory. Use `codey --continue` to resume with full context restoration.

## License

MIT
