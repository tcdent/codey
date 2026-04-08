#!/bin/bash
# Convenience script to run codey in Docker
#
# Usage:
#   ./run.sh                    # Start new session
#   ./run.sh --continue         # Continue previous session
#   ./run.sh --model opus       # Use specific model

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Check for API key
if [ -z "$ANTHROPIC_API_KEY" ]; then
    echo "Error: ANTHROPIC_API_KEY environment variable is not set"
    echo "Export it with: export ANTHROPIC_API_KEY=your-key-here"
    exit 1
fi

# Create local directories if they don't exist
mkdir -p workspace config data

# Build if image doesn't exist
if ! docker image inspect codey:latest &>/dev/null; then
    echo "Building codey image..."
    docker compose build
fi

# Run codey with all arguments passed through
exec docker compose run --rm codey "$@"
