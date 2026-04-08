# Codey Docker Container

Run codey in a containerized environment with all dependencies included.

## Quick Start

### 1. Set your API key

```bash
export ANTHROPIC_API_KEY=your-api-key-here
```

### 2. Build and run

```bash
cd container

# Build the image
docker compose build

# Run interactively
docker compose run --rm codey
```

## Usage

### Running with Docker Compose (recommended)

```bash
# Start an interactive session
docker compose run --rm codey

# Continue a previous session
docker compose run --rm codey --continue

# Specify a model
docker compose run --rm codey --model claude-sonnet-4-20250514
```

### Running with Docker directly

```bash
# Build the image
docker build -t codey:latest -f container/Dockerfile .

# Run interactively
docker run -it --rm \
  -e ANTHROPIC_API_KEY \
  -v $(pwd):/work \
  -v ~/.config/codey:/home/codey/.config/codey \
  --shm-size=2gb \
  codey:latest
```

## Configuration

### Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `ANTHROPIC_API_KEY` | Anthropic API key | Yes |
| `OPENROUTER_API_KEY` | OpenRouter API key (alternative) | No |
| `BRAVE_API_KEY` | Brave Search API key for web search | No |
| `TZ` | Timezone (e.g., `America/New_York`) | No |
| `CODEY_WORK_DIR` | Host path to mount as working directory | No |
| `CODEY_CONFIG_DIR` | Host path for codey configuration | No |
| `CODEY_DATA_DIR` | Host path for session transcripts | No |

### Volume Mounts

The container uses several volume mounts:

- `/work` - Your working directory (code to work on)
- `/home/codey/.config/codey` - Codey configuration
- `/work/.codey` - Session transcripts for `--continue` feature
- `/home/codey/.gitconfig` - Git configuration (read-only)

### Custom Configuration

Create a config file at `./config/config.toml`:

```toml
# Model configuration
model = "claude-sonnet-4-20250514"

# Chrome executable (already set in container)
# chrome_executable = "/usr/bin/chromium-browser"

# Auto-approve patterns (use with caution)
# auto_approve = ["Read*", "Glob*"]
```

## Building for Different Architectures

### Build for ARM64 (Apple Silicon, etc.)

Use Docker buildx for multi-arch builds:

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t codey:latest -f container/Dockerfile .
```

The Debian-based image supports both amd64 and arm64 natively.

## Included Tools

The container includes:

- **Chromium** - Headless browser for web content extraction
- **Git** - Version control operations
- **Bash** - Shell command execution
- **Neovim** - Optional IDE integration

## Extending the Image

You can create custom images based on codey for project-specific needs.

### Adding Custom Tools

```dockerfile
FROM codey:latest

USER root

# Install additional tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 \
    python3-pip \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Install a specific CLI tool
RUN npm install -g typescript

USER codey
```

### Adding a Project System Prompt

```dockerfile
FROM codey:latest

# Add a project-specific system prompt
COPY SYSTEM.md /home/codey/.config/codey/SYSTEM.md
```

Your `SYSTEM.md` might contain:

```markdown
You are working on the Acme project, a REST API built with Rust and Actix-web.

Key conventions:
- All handlers go in src/handlers/
- Use the existing error types in src/errors.rs
- Run `cargo test` before committing
```

### Full Example: Custom Project Image

```dockerfile
FROM codey:latest

USER root

# Install project-specific dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    postgresql-client \
    redis-tools \
    && rm -rf /var/lib/apt/lists/*

# Add custom scripts
COPY --chmod=755 scripts/deploy.sh /usr/local/bin/deploy

USER codey

# Add project system prompt
COPY --chown=codey:codey SYSTEM.md /home/codey/.config/codey/SYSTEM.md

# Add project config
COPY --chown=codey:codey config.toml /home/codey/.config/codey/config.toml
```

Build and use:

```bash
docker build -t my-project-codey .
docker run -it --rm -e ANTHROPIC_API_KEY -v $(pwd):/work my-project-codey
```

## Troubleshooting

### Chromium fails to start

Ensure adequate shared memory:

```bash
docker run --shm-size=2gb ...
```

### Permission denied errors

The container runs as non-root user `codey`. Ensure mounted volumes have appropriate permissions:

```bash
# Fix ownership if needed
sudo chown -R $(id -u):$(id -g) ./workspace ./config ./data
```

### Session not persisting

Ensure the data volume is properly mounted:

```bash
docker compose run --rm \
  -v $(pwd)/data:/work/.codey \
  codey --continue
```

## Security Notes

- The container runs as a non-root user by default
- Unnecessary capabilities are dropped
- Consider using read-only mounts where possible
- Never expose the container's ports to the network

## Development

To rebuild after code changes:

```bash
docker compose build --no-cache
```

To run with local source mounted (for development):

```bash
docker run -it --rm \
  -v $(pwd):/build \
  -w /build \
  rust:1.83-slim-bookworm \
  sh -c "apt-get update && apt-get install -y libssl-dev pkg-config git make patch && make build"
```
