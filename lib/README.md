# Patched Dependencies

## Patches

### ratatui-core
SIMD-accelerated buffer diffing for improved render performance.

### genai
Adds `extra_headers` support to the Anthropic adapter, enabling the
`interleaved-thinking-2025-05-14` beta header for extended thinking
between tool calls.

## Usage

```bash
make build              # Build with patches (default)
make SIMD=0 build       # Build with upstream crates
make profile            # Build release and run samply
```

## Structure

```
lib/
├── patches/
│   ├── ratatui-core/
│   │   ├── apply.sh              # Fetches and patches source
│   │   ├── Cargo.toml.template
│   │   ├── buffer-mod.patch
│   │   ├── simd-diff.patch       # AVX2/SSE4.1/NEON implementations
│   │   └── buffer-diff.patch
│   └── genai/
│       ├── apply.sh              # Fetches and patches source
│       └── anthropic-extra-headers.patch
├── ratatui-core/                 # Generated (gitignored)
└── genai/                        # Generated (gitignored)
```

## Adding a New Patch

1. Create `lib/patches/<lib-name>/apply.sh`
2. Add patch files to the same directory
3. Add target to Makefile

## Platforms (ratatui-core SIMD)

- **x86_64**: AVX2 (32-byte) or SSE4.1 (16-byte), runtime detection
- **aarch64**: NEON (16-byte) - Apple Silicon, ARM Linux

## Updating

1. Change the tag in `lib/patches/<lib-name>/apply.sh`
2. Run `make clean && make patch`
3. Fix any patch conflicts
