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

Modify the Dockerfile target architecture:

```dockerfile
# In the builder stage, change:
CARGO_BUILD_TARGET=aarch64-unknown-linux-musl
```

Or use Docker buildx for multi-arch builds:

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t codey:latest .
```

## Included Tools

The container includes:

- **Chromium** - Headless browser for web content extraction
- **Git** - Version control operations
- **Bash** - Shell command execution
- **Neovim** - Optional IDE integration

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
  rust:1.83-alpine \
  sh -c "apk add musl-dev openssl-dev git make patch perl && make build"
```
