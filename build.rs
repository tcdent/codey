use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let marker = Path::new("lib/ratatui-core/.patched");
    let script = Path::new("lib/apply-patches.sh");

    // Check if we need to run the patch:
    // 1. Marker doesn't exist (never patched)
    // 2. Script is newer than marker (patch was updated)
    let needs_patch = if !marker.exists() {
        true
    } else if let (Ok(marker_meta), Ok(script_meta)) = (fs::metadata(marker), fs::metadata(script))
    {
        if let (Ok(marker_time), Ok(script_time)) =
            (marker_meta.modified(), script_meta.modified())
        {
            script_time > marker_time
        } else {
            false
        }
    } else {
        false
    };

    if needs_patch {
        eprintln!("Applying ratatui-core SIMD patch...");

        let status = Command::new("bash")
            .arg("lib/apply-patches.sh")
            .status()
            .expect("Failed to run lib/apply-patches.sh - ensure bash is available");

        if !status.success() {
            panic!("lib/apply-patches.sh failed with status: {}", status);
        }
    }

    // Rerun build.rs if patch script or marker changes
    println!("cargo:rerun-if-changed=lib/apply-patches.sh");
    println!("cargo:rerun-if-changed=lib/ratatui-core/.patched");
}
