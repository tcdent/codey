# Codey Docker Container
# Multi-stage build for minimal runtime image

# =============================================================================
# Builder Stage - Compile codey
# =============================================================================
FROM rust:1.83-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl-dev \
    pkg-config \
    git \
    make \
    patch \
    perl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Set up the build environment
WORKDIR /build

# Copy the source code
COPY . .

# Apply SIMD patches and build
RUN make patch && make release

# =============================================================================
# Runtime Stage - Debian with Chromium
# =============================================================================
FROM debian:bookworm-slim

# Install runtime dependencies
# - chromium: for web content extraction (fetch_html tool)
# - bash: for shell tool execution
# - git: commonly needed for code operations
# - neovim: optional IDE integration
# - ca-certificates: for HTTPS requests
RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium \
    bash \
    git \
    neovim \
    ca-certificates \
    curl \
    openssh-client \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -d /home/codey -s /bin/bash codey

# Set up directories
RUN mkdir -p /home/codey/.config/codey /work && \
    chown -R codey:codey /home/codey /work

# Copy the compiled binary from builder
COPY --from=builder /build/target/release/codey /usr/local/bin/codey
RUN chmod +x /usr/local/bin/codey

# Set environment variables
ENV HOME=/home/codey
ENV CHROME_BIN=/usr/bin/chromium
ENV CHROMIUM_FLAGS="--no-sandbox --headless --disable-gpu --disable-dev-shm-usage"

# Switch to non-root user
USER codey
WORKDIR /work

# Default entrypoint
ENTRYPOINT ["codey"]
