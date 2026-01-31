# Codey Docker Container
# Multi-stage build for minimal runtime image

# =============================================================================
# Builder Stage - Compile codey with musl for static linking
# =============================================================================
FROM rust:1.83-alpine AS builder

# Install build dependencies
RUN apk add --no-cache \
    musl-dev \
    openssl-dev \
    openssl-libs-static \
    pkgconfig \
    git \
    make \
    patch \
    perl

# Set up the build environment
WORKDIR /build

# Copy the source code
COPY . .

# Apply SIMD patches and build
# Use vendored-openssl for static linking
RUN make patch && \
    CARGO_BUILD_TARGET=x86_64-unknown-linux-musl \
    EXTRA_FEATURES=vendored-openssl \
    make release

# =============================================================================
# Runtime Stage - Minimal Alpine with Chromium
# =============================================================================
FROM alpine:3.21

# Install runtime dependencies
# - chromium: for web content extraction (fetch_html tool)
# - bash: for shell tool execution
# - git: commonly needed for code operations
# - neovim: optional IDE integration
# - ca-certificates: for HTTPS requests
# - tzdata: timezone support
RUN apk add --no-cache \
    chromium \
    bash \
    git \
    neovim \
    ca-certificates \
    tzdata \
    # Chromium dependencies for headless operation
    nss \
    freetype \
    harfbuzz \
    ttf-freefont

# Create non-root user for security
RUN adduser -D -h /home/codey codey

# Set up directories
RUN mkdir -p /home/codey/.config/codey /work && \
    chown -R codey:codey /home/codey /work

# Copy the compiled binary from builder
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/codey /usr/local/bin/codey
RUN chmod +x /usr/local/bin/codey

# Set environment variables
ENV HOME=/home/codey
ENV CHROME_BIN=/usr/bin/chromium-browser
ENV CHROMIUM_FLAGS="--no-sandbox --headless --disable-gpu --disable-dev-shm-usage"

# Switch to non-root user
USER codey
WORKDIR /work

# Default entrypoint
ENTRYPOINT ["codey"]
