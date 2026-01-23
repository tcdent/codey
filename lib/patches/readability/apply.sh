#!/usr/bin/env bash
set -euo pipefail

PATCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LIB_DIR="$(dirname "$(dirname "$PATCH_DIR")")"
OUTPUT_DIR="$LIB_DIR/readability"
READABILITY_BRANCH="master"  # v0.3.0 - no tag exists for this version

[ -d "$OUTPUT_DIR" ] && rm -rf "$OUTPUT_DIR"

TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo "Fetching readability ($READABILITY_BRANCH)..."
git clone --depth 1 --branch "$READABILITY_BRANCH" --quiet \
    https://github.com/kumabook/readability.git "$TEMP_DIR/readability" 2>/dev/null
cp -r "$TEMP_DIR/readability" "$OUTPUT_DIR"

echo "Applying patches..."
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/remove-twitter-unlikely.patch"
patch -d "$OUTPUT_DIR" -p1 -s < "$PATCH_DIR/list-page-detection.patch"

touch "$OUTPUT_DIR/.patched"
echo "Done."
