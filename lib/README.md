# Patched Dependencies

This directory contains patches for upstream dependencies and infrastructure to apply them.

## Structure

```
lib/
├── README.md           # This file
├── apply-patches.sh    # Script to fetch and patch dependencies
├── patches/            # Patch files
│   └── ratatui-core-simd-diff.patch
└── ratatui-core/       # Generated: patched ratatui-core source
```

## Usage

### Initial Setup

```bash
# Fetch and patch dependencies
./lib/apply-patches.sh
```

### After Updating Patches

```bash
# Re-apply patches (will prompt to confirm)
./lib/apply-patches.sh

# Force re-apply
rm -rf lib/ratatui-core && ./lib/apply-patches.sh
```

### Cargo Configuration

After running the patch script, add to `Cargo.toml`:

```toml
[patch.crates-io]
ratatui-core = { path = "lib/ratatui-core", features = ["simd-diff"] }
```

## Patches

### ratatui-core-simd-diff.patch

Adds SIMD-accelerated buffer diffing to ratatui-core.

**What it does:**
- Adds a `simd-diff` feature flag
- Implements `find_changed_ranges()` using AVX2/SSE4.1 (x86_64) or NEON (aarch64)
- Modified `Buffer::diff()` to use SIMD to quickly identify unchanged regions
- Falls back to scalar comparison for actual changed cells

**Expected performance improvement:**
- 2-4x faster for mostly-static UIs
- 4-8x faster for large buffers with few changes

**Limitations:**
- NEON (ARM) implementation is a stub - needs implementation
- Requires nightly Rust for some SIMD intrinsics (or use `std::simd` when stable)

## Upstream Contributions

These patches are candidates for upstream contribution:

1. **SIMD diff** - See [ratatui issue #1116](https://github.com/ratatui/ratatui/issues/1116)
   - Status: Open discussion about diff performance
   - Approach: Could submit as opt-in feature

## Maintenance

When updating ratatui version:

1. Update `RATATUI_CORE_VERSION` in `apply-patches.sh`
2. Re-run `./lib/apply-patches.sh`
3. Fix any patch conflicts
4. Test thoroughly

## Development

To modify patches:

1. Make changes in `lib/ratatui-core/`
2. Create new patch: `git diff > patches/new-feature.patch`
3. Test by re-running `apply-patches.sh` on fresh clone
