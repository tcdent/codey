#!/usr/bin/env bash
set -euo pipefail

PATCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB_DIR="$(dirname "$(dirname "$PATCH_DIR")")"
OUTPUT_DIR="$LIB_DIR/genai"
# Use 0.5.1 release as base (latest stable)
GENAI_TAG="v0.5.1"

[ -d "$OUTPUT_DIR" ] && rm -rf "$OUTPUT_DIR"

TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo "Fetching genai ($GENAI_TAG)..."
git clone --depth 1 --branch "$GENAI_TAG" --quiet \
    https://github.com/jeremychone/rust-genai.git "$TEMP_DIR/genai" 2>/dev/null
cp -r "$TEMP_DIR/genai" "$OUTPUT_DIR"

echo "Applying patches..."
# Thinking blocks with signatures for extended thinking + tool use
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/thinking-blocks-signatures.patch"

# OAuth support for Authorization header detection
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/oauth-support.patch"

touch "$OUTPUT_DIR/.patched"
echo "Done."
