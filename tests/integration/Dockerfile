# Build stage: compile codey from source
FROM rust:1.85-bookworm AS builder

WORKDIR /build
COPY . .

# Build with CLI features. SIMD=0 skips the ratatui-core patching step
# (needs `patch` + git clone of ratatui) since it's a rendering optimization
# we don't need in the test environment.
RUN apt-get update && apt-get install -y pkg-config libssl-dev \
    && SIMD=0 make release

# Runtime stage: minimal image with test tooling
FROM debian:bookworm-slim

# System dependencies for codey + test harness
RUN apt-get update && apt-get install -y --no-install-recommends \
    tmux \
    git \
    python3 \
    python3-pip \
    python3-venv \
    chromium \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Chromium flags for containerized/headless use
ENV CHROME_PATH=/usr/bin/chromium
ENV CHROMIUM_FLAGS="--no-sandbox --disable-gpu --headless"

# Install codey binary
COPY --from=builder /build/target/release/codey /usr/local/bin/codey

# Set up Python test environment
WORKDIR /test
COPY tests/integration/requirements.txt /test/requirements.txt
RUN python3 -m venv /test/venv \
    && /test/venv/bin/pip install --no-cache-dir -r requirements.txt
ENV PATH="/test/venv/bin:$PATH"

# Copy test harness
COPY tests/integration/ /test/

# Create a workspace directory that tests will operate in
RUN mkdir -p /workspace && \
    cd /workspace && git init && \
    git config user.email "test@codey.dev" && \
    git config user.name "Codey Test" && \
    echo "# Test Workspace" > README.md && \
    git add . && git commit -m "init"

WORKDIR /workspace

ENTRYPOINT ["pytest", "/test/", "-v", "--tb=short"]
