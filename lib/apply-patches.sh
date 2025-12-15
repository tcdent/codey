#!/usr/bin/env bash
#
# Apply patches to vendored dependencies
#
# This script fetches ratatui-core at a specific version and applies
# our performance patches.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB_DIR="$SCRIPT_DIR"
PATCHES_DIR="$LIB_DIR/patches"

# ratatui-core version to patch (should match Cargo.lock)
RATATUI_CORE_VERSION="0.1.0-beta.0"
RATATUI_CORE_REPO="https://github.com/ratatui/ratatui.git"
RATATUI_CORE_TAG="ratatui-core-v${RATATUI_CORE_VERSION}"

# Output directory
RATATUI_CORE_DIR="$LIB_DIR/ratatui-core"

echo "=== Codey Dependency Patcher ==="
echo ""

# Check if we need to fetch
if [ -d "$RATATUI_CORE_DIR" ]; then
    echo "ratatui-core already exists at $RATATUI_CORE_DIR"
    read -p "Re-fetch and re-patch? [y/N] " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Skipping. Run with --force to override."
        exit 0
    fi
    rm -rf "$RATATUI_CORE_DIR"
fi

echo "Fetching ratatui-core $RATATUI_CORE_VERSION..."

# Create temp directory for clone
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

# Sparse checkout just ratatui-core
git clone --depth 1 --filter=blob:none --sparse \
    "$RATATUI_CORE_REPO" "$TEMP_DIR/ratatui" 2>/dev/null || {
    echo "Failed to clone. Trying without sparse checkout..."
    git clone --depth 1 "$RATATUI_CORE_REPO" "$TEMP_DIR/ratatui"
}

cd "$TEMP_DIR/ratatui"

# Try to checkout specific tag, fall back to main
git fetch --depth 1 origin "refs/tags/$RATATUI_CORE_TAG" 2>/dev/null && \
    git checkout FETCH_HEAD 2>/dev/null || {
    echo "Tag $RATATUI_CORE_TAG not found, using main branch"
}

# Enable sparse checkout and get just ratatui-core
git sparse-checkout set ratatui-core 2>/dev/null || true

# Copy ratatui-core to our lib directory
if [ -d "ratatui-core" ]; then
    cp -r ratatui-core "$RATATUI_CORE_DIR"
else
    echo "Error: ratatui-core directory not found in repo"
    exit 1
fi

cd "$LIB_DIR"

echo ""
echo "Applying patches..."

# Apply each patch
for patch in "$PATCHES_DIR"/*.patch; do
    if [ -f "$patch" ]; then
        patch_name=$(basename "$patch")
        echo "  Applying $patch_name..."

        # Determine target directory from patch name
        if [[ "$patch_name" == ratatui-core* ]]; then
            target_dir="$RATATUI_CORE_DIR"
        else
            echo "    Unknown target for $patch_name, skipping"
            continue
        fi

        # Apply patch (allow partial failures for now during development)
        cd "$target_dir"
        if patch -p1 --forward < "$patch" 2>/dev/null; then
            echo "    Applied successfully"
        else
            echo "    Patch may have already been applied or needs adjustment"
        fi
        cd "$LIB_DIR"
    fi
done

echo ""
echo "Adding simd-diff feature to ratatui-core Cargo.toml..."

# Add the simd-diff feature if not present
CARGO_TOML="$RATATUI_CORE_DIR/Cargo.toml"
if [ -f "$CARGO_TOML" ]; then
    if ! grep -q "simd-diff" "$CARGO_TOML"; then
        # Add feature after [features] section
        sed -i '/\[features\]/a simd-diff = []' "$CARGO_TOML" 2>/dev/null || \
        echo 'simd-diff = []' >> "$CARGO_TOML"
        echo "  Added simd-diff feature"
    else
        echo "  simd-diff feature already present"
    fi
fi

echo ""
echo "=== Patching complete ==="
echo ""
echo "Add to your Cargo.toml:"
echo ""
echo '[patch.crates-io]'
echo 'ratatui-core = { path = "lib/ratatui-core", features = ["simd-diff"] }'
echo ""
