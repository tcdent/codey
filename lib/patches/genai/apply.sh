#!/usr/bin/env bash
set -euo pipefail

PATCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB_DIR="$(dirname "$(dirname "$PATCH_DIR")")"
OUTPUT_DIR="$LIB_DIR/genai"
GENAI_TAG="v0.4.4"

[ -d "$OUTPUT_DIR" ] && rm -rf "$OUTPUT_DIR"

TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo "Fetching genai ($GENAI_TAG)..."
git clone --depth 1 --branch "$GENAI_TAG" --quiet \
    https://github.com/jeremychone/rust-genai.git "$TEMP_DIR/genai" 2>/dev/null
cp -r "$TEMP_DIR/genai" "$OUTPUT_DIR"

echo "Applying patches..."
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/anthropic-extra-headers.patch"
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/streamer-thinking-fix.patch"

touch "$OUTPUT_DIR/.patched"
echo "Done."
