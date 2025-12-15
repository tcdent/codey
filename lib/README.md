# Patched Dependencies

SIMD-accelerated buffer diffing for ratatui-core.

## Usage

```bash
./lib/apply-patches.sh          # Fetch and patch ratatui-core
cargo build                     # Build with SIMD (default)
cargo build --features no-simd  # Disable SIMD
```

## Structure

```
lib/
├── apply-patches.sh          # Fetches ratatui-core, applies patches
├── patches/                  # Diff files
│   ├── Cargo.toml.template
│   ├── buffer-mod.patch
│   ├── simd-diff.patch       # AVX2/SSE4.1/NEON implementations
│   └── buffer-diff.patch
└── ratatui-core/             # Generated (gitignored)
```

## Platforms

- **x86_64**: AVX2 (32-byte) or SSE4.1 (16-byte), runtime detection
- **aarch64**: NEON (16-byte) - Apple Silicon, ARM Linux

## Updating

1. Change `RATATUI_TAG` in `apply-patches.sh`
2. Run `./lib/apply-patches.sh`
3. Fix any patch conflicts
