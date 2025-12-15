#!/usr/bin/env bash
set -euo pipefail

PATCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB_DIR="$(dirname "$(dirname "$PATCH_DIR")")"
OUTPUT_DIR="$LIB_DIR/ratatui-core"
RATATUI_TAG="ratatui-core-v0.1.0-beta.0"

[ -d "$OUTPUT_DIR" ] && rm -rf "$OUTPUT_DIR"

TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo "Fetching ratatui-core ($RATATUI_TAG)..."
git clone --depth 1 --branch "$RATATUI_TAG" --quiet \
    https://github.com/ratatui/ratatui.git "$TEMP_DIR/ratatui" 2>/dev/null
cp -r "$TEMP_DIR/ratatui/ratatui-core" "$OUTPUT_DIR"

echo "Applying patches..."
cp "$PATCH_DIR/Cargo.toml.template" "$OUTPUT_DIR/Cargo.toml"
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/buffer-mod.patch"
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/simd-diff.patch"
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/buffer-diff.patch"

touch "$OUTPUT_DIR/.patched"
echo "Done."
