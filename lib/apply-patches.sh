#!/usr/bin/env bash
# Apply SIMD diff patch to ratatui-core
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_DIR="$SCRIPT_DIR/ratatui-core"

echo "=== Codey Dependency Patcher ==="

[ -d "$OUTPUT_DIR" ] && rm -rf "$OUTPUT_DIR"

TEMP_DIR=$(mktemp -d)
trap "rm -rf $TEMP_DIR" EXIT

echo "Fetching ratatui (ratatui-core-v0.1.0-beta.0)..."
git clone --depth 1 --branch ratatui-core-v0.1.0-beta.0 --quiet https://github.com/ratatui/ratatui.git "$TEMP_DIR/ratatui"
cp -r "$TEMP_DIR/ratatui/ratatui-core" "$OUTPUT_DIR"

echo "Patching Cargo.toml..."
cat > "$OUTPUT_DIR/Cargo.toml" << 'EOF'
[package]
name = "ratatui-core"
version = "0.1.0-beta.0"
edition = "2021"
license = "MIT"

[features]
default = []
no-simd = []
std = ["itertools/use_std", "thiserror/std", "kasuari/std", "compact_str/std", "unicode-truncate/std", "strum/std"]
layout-cache = ["std"]
underline-color = []
scrolling-regions = []

[dependencies]
bitflags = "2.6"
compact_str = { version = "0.8", default-features = false }
hashbrown = "0.15"
indoc = "2.0"
itertools = { version = "0.14", default-features = false }
kasuari = { version = "0.4", default-features = false }
lru = "0.12"
strum = { version = "0.27", default-features = false, features = ["derive"] }
thiserror = { version = "2.0", default-features = false }
unicode-segmentation = "1.12"
unicode-truncate = { version = "1.1", default-features = false }
unicode-width = "0.2"

[dev-dependencies]
pretty_assertions = "1.4"
EOF

echo "Adding simd_diff module declaration to src/buffer.rs..."
# Insert after inner attributes, before other mod declarations
python3 << 'MODPATCH'
path = "/home/user/codey/lib/ratatui-core/src/buffer.rs"
with open(path, 'r') as f:
    content = f.read()
decl = """#[cfg(all(not(feature = "no-simd"), any(target_arch = "x86_64", target_arch = "aarch64")))]
mod simd_diff;

"""
content = content.replace("mod assert;", decl + "mod assert;")
with open(path, 'w') as f:
    f.write(content)
print("Module declaration added")
MODPATCH

echo "Creating src/buffer/simd_diff.rs..."
cat > "$OUTPUT_DIR/src/buffer/simd_diff.rs" << 'EOF'
extern crate std;
use std::vec::Vec;
use std::vec;
#[cfg(target_arch = "x86_64")]
use std::is_x86_feature_detected;
use super::Cell;

pub fn find_changed_ranges(prev: &[Cell], curr: &[Cell]) -> Vec<(usize, usize)> {
    let len = prev.len().min(curr.len());
    if len < 128 { return vec![(0, len)]; }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx2") { return unsafe { avx2(prev, curr) }; }
        if is_x86_feature_detected!("sse4.1") { return unsafe { sse41(prev, curr) }; }
    }
    #[cfg(target_arch = "aarch64")]
    { return unsafe { neon(prev, curr) }; }

    #[allow(unreachable_code)]
    vec![(0, len)]
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn avx2(prev: &[Cell], curr: &[Cell]) -> Vec<(usize, usize)> {
    use core::arch::x86_64::*;
    let len = prev.len().min(curr.len());
    let (pp, cp) = (prev.as_ptr() as *const u8, curr.as_ptr() as *const u8);
    let cs = core::mem::size_of::<Cell>();
    let bl = len * cs;
    let (mut r, mut s, mut o) = (Vec::new(), None::<usize>, 0);
    while o + 32 <= bl {
        let m = _mm256_movemask_epi8(_mm256_cmpeq_epi8(
            _mm256_loadu_si256(pp.add(o) as *const __m256i),
            _mm256_loadu_si256(cp.add(o) as *const __m256i)));
        let co = o / cs;
        if m != -1i32 { if s.is_none() { s = Some(co); } }
        else if let Some(st) = s.take() { r.push((st, co + 1)); }
        o += 32;
    }
    if let Some(st) = s { r.push((st, len)); }
    else if o/cs < len { for i in o/cs..len { if prev[i] != curr[i] { r.push((o/cs, len)); break; } } }
    merge(r)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn sse41(prev: &[Cell], curr: &[Cell]) -> Vec<(usize, usize)> {
    use core::arch::x86_64::*;
    let len = prev.len().min(curr.len());
    let (pp, cp) = (prev.as_ptr() as *const u8, curr.as_ptr() as *const u8);
    let cs = core::mem::size_of::<Cell>();
    let bl = len * cs;
    let (mut r, mut s, mut o) = (Vec::new(), None::<usize>, 0);
    while o + 16 <= bl {
        let m = _mm_movemask_epi8(_mm_cmpeq_epi8(
            _mm_loadu_si128(pp.add(o) as *const __m128i),
            _mm_loadu_si128(cp.add(o) as *const __m128i)));
        let co = o / cs;
        if m != 0xFFFF { if s.is_none() { s = Some(co); } }
        else if let Some(st) = s.take() { r.push((st, co + 1)); }
        o += 16;
    }
    if let Some(st) = s { r.push((st, len)); }
    merge(r)
}

#[cfg(target_arch = "aarch64")]
unsafe fn neon(prev: &[Cell], curr: &[Cell]) -> Vec<(usize, usize)> {
    use core::arch::aarch64::*;
    let len = prev.len().min(curr.len());
    let (pp, cp) = (prev.as_ptr() as *const u8, curr.as_ptr() as *const u8);
    let cs = core::mem::size_of::<Cell>();
    let bl = len * cs;
    let (mut r, mut s, mut o) = (Vec::new(), None::<usize>, 0);
    while o + 16 <= bl {
        let m = vminvq_u8(vceqq_u8(vld1q_u8(pp.add(o)), vld1q_u8(cp.add(o))));
        let co = o / cs;
        if m != 0xFF { if s.is_none() { s = Some(co); } }
        else if let Some(st) = s.take() { r.push((st, co + 1)); }
        o += 16;
    }
    if let Some(st) = s { r.push((st, len)); }
    merge(r)
}

fn merge(mut r: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if r.len() <= 1 { return r; }
    r.sort_by_key(|x| x.0);
    let mut m = vec![r[0]];
    for x in r.into_iter().skip(1) {
        let l = m.last_mut().unwrap();
        if x.0 <= l.1 + 8 { l.1 = l.1.max(x.1); } else { m.push(x); }
    }
    m
}
EOF

echo "Patching Buffer::diff()..."
python3 << 'PYPATCH'
import re

path = "/home/user/codey/lib/ratatui-core/src/buffer/buffer.rs"
with open(path, 'r') as f:
    content = f.read()

if 'diff_simd' in content:
    print("Already patched")
    exit(0)

# Pattern to match the diff function signature
pattern = r'(    pub fn diff<.a>\(&self, other: &.a Self\) -> Vec<\(u16, u16, &.a Cell\)> \{)'

replacement = '''    pub fn diff<'a>(&self, other: &'a Self) -> Vec<(u16, u16, &'a Cell)> {
        #[cfg(all(not(feature = "no-simd"), any(target_arch = "x86_64", target_arch = "aarch64")))]
        { return self.diff_simd(other); }
        #[cfg(any(feature = "no-simd", not(any(target_arch = "x86_64", target_arch = "aarch64"))))]
        { self.diff_scalar(other) }
    }

    #[cfg(all(not(feature = "no-simd"), any(target_arch = "x86_64", target_arch = "aarch64")))]
    fn diff_simd<'a>(&self, other: &'a Self) -> Vec<(u16, u16, &'a Cell)> {
        let (prev, next) = (&self.content, &other.content);
        let changed = super::simd_diff::find_changed_ranges(prev, next);
        let mut updates = Vec::with_capacity(changed.iter().map(|(s,e)| e-s).sum::<usize>()/4+1);
        for (rs, re) in changed {
            let (mut inv, mut skip) = (0usize, 0usize);
            for i in rs..re.min(next.len()).min(prev.len()) {
                let (cur, prv) = (&next[i], &prev[i]);
                if !cur.skip && (cur != prv || inv > 0) && skip == 0 {
                    let (x, y) = self.pos_of(i);
                    updates.push((x, y, &next[i]));
                    let (sym, w) = (cur.symbol(), cur.symbol().width());
                    if w > 1 && sym.chars().any(|c| c == '\\\\u{FE0F}') {
                        for k in 1..w {
                            let j = i + k;
                            if j >= next.len() || j >= prev.len() { break; }
                            if !next[j].skip && prev[j] != next[j] {
                                let (tx, ty) = self.pos_of(j);
                                updates.push((tx, ty, &next[j]));
                            }
                        }
                    }
                }
                skip = cur.symbol().width().saturating_sub(1);
                inv = cmp::max(cur.symbol().width(), prv.symbol().width()).max(inv).saturating_sub(1);
            }
        }
        updates
    }

    #[cfg(any(feature = "no-simd", not(any(target_arch = "x86_64", target_arch = "aarch64"))))]
    fn diff_scalar<'a>(&self, other: &'a Self) -> Vec<(u16, u16, &'a Cell)> {'''

content = re.sub(pattern, replacement, content)

with open(path, 'w') as f:
    f.write(content)

print("Patched successfully")
PYPATCH

echo ""
echo "=== Done! Patched ratatui-core at: $OUTPUT_DIR ==="
