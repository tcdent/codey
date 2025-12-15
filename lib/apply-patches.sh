#!/usr/bin/env bash
# Apply SIMD diff patches to ratatui-core
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PATCHES_DIR="$SCRIPT_DIR/patches"
OUTPUT_DIR="$SCRIPT_DIR/ratatui-core"
RATATUI_TAG="ratatui-core-v0.1.0-beta.0"

echo "=== Codey Dependency Patcher ==="

# Clean existing output
[ -d "$OUTPUT_DIR" ] && rm -rf "$OUTPUT_DIR"

# Create temp dir for clone
TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo "Fetching ratatui ($RATATUI_TAG)..."
git clone --depth 1 --branch "$RATATUI_TAG" --quiet \
    https://github.com/ratatui/ratatui.git "$TEMP_DIR/ratatui" 2>/dev/null
cp -r "$TEMP_DIR/ratatui/ratatui-core" "$OUTPUT_DIR"

echo "Applying patches..."

# Replace Cargo.toml (complete replacement due to workspace inheritance)
echo "  - Cargo.toml.template"
cp "$PATCHES_DIR/Cargo.toml.template" "$OUTPUT_DIR/Cargo.toml"

# Apply buffer module declaration patch
echo "  - buffer-mod.patch"
patch -d "$OUTPUT_DIR" -p1 < "$PATCHES_DIR/buffer-mod.patch"

# Apply simd_diff.rs (new file)
echo "  - simd-diff.patch"
patch -d "$OUTPUT_DIR" -p1 < "$PATCHES_DIR/simd-diff.patch"

# Apply buffer diff function patch
echo "  - buffer-diff.patch"
patch -d "$OUTPUT_DIR" -p1 < "$PATCHES_DIR/buffer-diff.patch"

# Create marker file for build.rs caching
touch "$OUTPUT_DIR/.patched"

echo ""
echo "=== Done! Patched ratatui-core at: $OUTPUT_DIR ==="
