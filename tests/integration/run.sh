#!/usr/bin/env bash
# Run integration tests locally.
#
# Usage:
#   ANTHROPIC_API_KEY=sk-... ./tests/integration/run.sh
#   ANTHROPIC_API_KEY=sk-... ./tests/integration/run.sh test_read_file.py
#   ANTHROPIC_API_KEY=sk-... ./tests/integration/run.sh test_shell.py::test_shell_command_execution

set -euo pipefail
cd "$(dirname "$0")/../.."

if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    echo "Error: ANTHROPIC_API_KEY must be set"
    exit 1
fi

EXTRA_ARGS=""
if [ $# -gt 0 ]; then
    # Pass specific test files/selectors to pytest inside the container.
    EXTRA_ARGS="$*"
fi

docker compose -f tests/integration/docker-compose.yml build
docker compose -f tests/integration/docker-compose.yml run \
    --rm integration-test \
    ${EXTRA_ARGS:+pytest /test/ -v --tb=short -k "$EXTRA_ARGS"}
