//! Auto-update checker.
//!
//! Polls the GitHub Releases API periodically to detect new versions.
//! When a newer version is found, sends a notification through a channel
//! so the app can alert the user and offer to self-update.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/tcdent/codey/releases/latest";

/// How often to check for updates (1 hour).
const CHECK_INTERVAL: Duration = Duration::from_secs(3600);

/// Initial delay before first check so startup isn't slowed.
const INITIAL_DELAY: Duration = Duration::from_secs(30);

/// Info about an available update.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub download_url: Option<String>,
    pub release_url: String,
}

/// Determine the expected asset name for this platform.
fn asset_name() -> Option<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Some("codey-darwin-arm64"),
        ("linux", "x86_64") => Some("codey-linux-x86_64"),
        ("linux", "aarch64") => Some("codey-linux-arm64"),
        _ => None,
    }
}

/// Parse a version string, stripping a leading 'v' if present.
fn parse_version(s: &str) -> Option<semver::Version> {
    let s = s.strip_prefix('v').unwrap_or(s);
    semver::Version::parse(s).ok()
}

/// Check the GitHub Releases API once and return update info if a newer version exists.
async fn check_once(client: &reqwest::Client) -> Option<UpdateInfo> {
    let resp = client
        .get(GITHUB_RELEASES_URL)
        .header("User-Agent", format!("Codey/{}", APP_VERSION))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        tracing::debug!("Update check got status {}", resp.status());
        return None;
    }

    let body: serde_json::Value = resp.json().await.ok()?;

    let tag = body.get("tag_name")?.as_str()?;
    let latest = parse_version(tag)?;
    let current = parse_version(APP_VERSION)?;

    if latest <= current {
        tracing::debug!("Up to date: current={} latest={}", current, latest);
        return None;
    }

    // Find the download URL for our platform's asset.
    let download_url = asset_name().and_then(|name| {
        let tar_name = format!("{}.tar.gz", name);
        body.get("assets")?
            .as_array()?
            .iter()
            .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(&tar_name))
            .and_then(|a| a.get("browser_download_url")?.as_str())
            .map(String::from)
    });

    let release_url = body
        .get("html_url")
        .and_then(|u| u.as_str())
        .unwrap_or("https://github.com/tcdent/codey/releases/latest")
        .to_string();

    Some(UpdateInfo {
        current: current.to_string(),
        latest: latest.to_string(),
        download_url,
        release_url,
    })
}

/// Spawn a background task that periodically checks for updates.
///
/// Returns a receiver that will yield `UpdateInfo` when a new version is found.
/// Only sends once per new version detected (won't spam).
pub fn spawn_checker() -> mpsc::Receiver<UpdateInfo> {
    let (tx, rx) = mpsc::channel(1);

    tokio::spawn(async move {
        tokio::time::sleep(INITIAL_DELAY).await;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        loop {
            tracing::debug!("Checking for updates...");
            if let Some(info) = check_once(&client).await {
                tracing::info!(
                    "Update available: {} -> {}",
                    info.current,
                    info.latest
                );
                // Send once; if the channel is full or closed, stop checking.
                if tx.send(info).await.is_err() {
                    break;
                }
                // After notifying, stop polling — one notification is enough.
                break;
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });

    rx
}

/// Download and replace the current binary with the updated version.
///
/// 1. Downloads the .tar.gz asset from GitHub
/// 2. Extracts the binary to a temp file next to the current executable
/// 3. Atomically replaces the current executable
pub async fn self_update(download_url: &str) -> anyhow::Result<PathBuf> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;

    tracing::info!("Downloading update from {}", download_url);
    let resp = client
        .get(download_url)
        .header("User-Agent", format!("Codey/{}", APP_VERSION))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Download failed with status {}", resp.status());
    }

    let bytes = resp.bytes().await?;

    // Decompress gzip, then extract from tar
    let gz = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(gz);

    let current_exe = env::current_exe()?;
    let parent = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine executable directory"))?;

    let tmp_path = parent.join(".codey_update_tmp");

    // Extract the single binary from the archive
    let mut found = false;
    for entry in archive.entries()? {
        let mut entry = entry?;
        // The archive contains a single file named like "codey-darwin-arm64"
        entry.unpack(&tmp_path)?;
        found = true;
        break; // Only one file in the archive
    }

    if !found {
        anyhow::bail!("No binary found in release archive");
    }

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755))?;
    }

    // Atomic replace: rename the temp file over the current executable.
    // On Unix this works even while the binary is running.
    std::fs::rename(&tmp_path, &current_exe)?;

    tracing::info!("Binary updated at {}", current_exe.display());
    Ok(current_exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(
            parse_version("v0.1.0-rc.12"),
            Some(semver::Version::parse("0.1.0-rc.12").unwrap())
        );
        assert_eq!(
            parse_version("0.2.0"),
            Some(semver::Version::parse("0.2.0").unwrap())
        );
        assert_eq!(
            parse_version("v1.0.0"),
            Some(semver::Version::parse("1.0.0").unwrap())
        );
    }

    #[test]
    fn test_asset_name_is_valid() {
        // Just ensure it doesn't panic on this platform
        let _ = asset_name();
    }

    #[test]
    fn test_parse_version_invalid() {
        assert!(parse_version("not-a-version").is_none());
        assert!(parse_version("").is_none());
    }
}
