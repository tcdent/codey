# Patched Dependencies

SIMD-accelerated buffer diffing for ratatui-core.

## Usage

```bash
make build              # Build with SIMD patch (default)
make SIMD=0 build       # Build with upstream crates
make profile            # Build release and run samply
```

## Structure

```
lib/
├── patches/
│   └── ratatui-core/
│       ├── apply.sh              # Fetches and patches source
│       ├── Cargo.toml.template
│       ├── buffer-mod.patch
│       ├── simd-diff.patch       # AVX2/SSE4.1/NEON implementations
│       └── buffer-diff.patch
└── ratatui-core/                 # Generated (gitignored)
```

## Adding a New Patch

1. Create `lib/patches/<lib-name>/apply.sh`
2. Add patch files to the same directory
3. Add target to Makefile

## Platforms

- **x86_64**: AVX2 (32-byte) or SSE4.1 (16-byte), runtime detection
- **aarch64**: NEON (16-byte) - Apple Silicon, ARM Linux

## Updating

1. Change `RATATUI_TAG` in `lib/patches/ratatui-core/apply.sh`
2. Run `make clean && make patch`
3. Fix any patch conflicts
