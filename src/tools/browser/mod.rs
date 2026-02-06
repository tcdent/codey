//! Browser automation module
//!
//! Provides headless browser functionality for fetching JavaScript-rendered pages
//! and persistent interactive browser sessions.
//! Uses Chrome/Chromium via the chromiumoxide crate.
//!
//! # Features
//!
//! - Headless browser page rendering with JavaScript support
//! - Chrome profile support for authenticated sessions
//! - Readability-based content extraction
//! - HTML to Markdown conversion for token-efficient output
//! - Persistent browser sessions with named access (see [`session`] module)
//!
//! # Chrome Profile Authentication
//!
//! To access authenticated sessions (logged-in state), we copy the user's Chrome profile
//! to an isolated temp directory. This is necessary because:
//!
//! 1. **SingletonLock**: Chrome locks its user data directory at the root level, not per-profile.
//!    Having *any* Chrome window open locks `~/Library/Application Support/Google/Chrome/`.
//!    By copying to `/tmp/codey-browser-{pid}/`, we avoid this conflict.
//!
//! 2. **Cookie Encryption**: Chrome encrypts cookies using macOS Keychain. The encryption key
//!    is stored in Keychain under "Chrome Safe Storage" and is shared across all profiles
//!    (it's per-application, not per-profile). This means copied cookies can be decrypted
//!    as long as Chrome has Keychain access.
//!
//! 3. **Keychain Access**: chromiumoxide's default args include `--password-store=basic` and
//!    `--use-mock-keychain`, which tell Chrome to use a mock keychain instead of the real one.
//!    This breaks cookie decryption! We must disable these defaults when using a profile.

pub mod session;

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use crate::config::BrowserConfig as AppBrowserConfig;

// TODO: Refactor to avoid global state. See issue #46.
// We use OnceLock here because tool handlers don't have access to app config.

/// Browser configuration context, initialized at app startup
static BROWSER_CONTEXT: OnceLock<BrowserContext> = OnceLock::new();

/// Context for browser-based tools (fetch_html, fetch_screenshot)
#[derive(Debug, Clone)]
pub struct BrowserContext {
    pub chrome_executable: Option<String>,
    pub chrome_user_data_dir: Option<String>,
    pub chrome_profile: Option<String>,
    pub headless: bool,
    pub viewport_width: u32,
    pub viewport_height: u32,
    pub page_load_wait_ms: u64,
}

impl Default for BrowserContext {
    fn default() -> Self {
        Self {
            chrome_executable: None,
            chrome_user_data_dir: None,
            chrome_profile: None,
            headless: true,
            viewport_width: 800,
            viewport_height: 4000,
            page_load_wait_ms: 10000,
        }
    }
}

/// Initialize browser context from config. Called once at app startup.
pub fn init_browser_context(config: &AppBrowserConfig) {
    let user_data_dir = config.chrome_user_data_dir.as_ref().and_then(|p| {
        let path_str = p.to_str()?;
        // Expand ~ to home directory
        if path_str.starts_with("~/") {
            dirs::home_dir().map(|home| home.join(&path_str[2..]).to_string_lossy().to_string())
        } else {
            Some(path_str.to_string())
        }
    });

    BROWSER_CONTEXT
        .set(BrowserContext {
            chrome_executable: config
                .chrome_executable
                .as_ref()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string()),
            chrome_user_data_dir: user_data_dir,
            chrome_profile: config.chrome_profile.clone(),
            headless: config.headless,
            viewport_width: config.viewport_width,
            viewport_height: config.viewport_height,
            page_load_wait_ms: config.page_load_wait_ms,
        })
        .ok();
}

/// Get the browser context, if initialized.
pub(crate) fn browser_context() -> Option<&'static BrowserContext> {
    BROWSER_CONTEXT.get()
}

/// Result of fetch_html operation
#[derive(Debug)]
pub struct FetchHtmlResult {
    pub content: String,
    pub title: Option<String>,
    pub url: String,
}

/// Detect available Chrome/Chromium browser
pub fn detect_browser() -> Option<String> {
    let candidates = [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];

    for candidate in candidates {
        if which_browser(candidate).is_some() {
            return Some(candidate.to_string());
        }
    }

    None
}

/// Check if a browser executable exists in PATH or as absolute path
fn which_browser(name: &str) -> Option<String> {
    // If it's an absolute path, check directly
    if name.starts_with('/') {
        if Path::new(name).exists() {
            return Some(name.to_string());
        }
        return None;
    }

    // Check in PATH
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let full_path = Path::new(dir).join(name);
            if full_path.exists() {
                return Some(full_path.to_string_lossy().to_string());
            }
        }
    }

    None
}

/// Fetch HTML content using headless browser and convert to readable markdown
///
/// This is the one-shot convenience function. It launches a browser, navigates,
/// waits for the page to settle, extracts readable content, and tears down the
/// browser — all using the same code path as persistent browser sessions.
///
/// Browser settings (executable path, profile) are read from the global
/// BrowserContext initialized at app startup.
pub async fn fetch_html(url: &str, max_length: Option<usize>) -> Result<FetchHtmlResult, String> {
    let max_length = max_length.unwrap_or(100000);

    // Validate URL
    let parsed_url = url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;

    if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
        return Err(format!(
            "Unsupported URL scheme: {}. Only http and https are allowed.",
            parsed_url.scheme()
        ));
    }

    // Launch browser, navigate, wait for settle — shared with session module
    let (mut browser, page, handler_task, temp_dir) = session::launch_browser(url).await?;

    // Extract content
    let content = session::extract_page_content(&page, url).await?;

    // Tear down immediately (one-shot, no persistent session)
    let _ = browser.close().await;
    handler_task.abort();
    if let Some(ref temp) = temp_dir {
        let _ = std::fs::remove_dir_all(temp);
    }

    // Truncate if needed
    let mut markdown = content.markdown;
    if markdown.len() > max_length {
        let mut end = max_length;
        while end > 0 && !markdown.is_char_boundary(end) {
            end -= 1;
        }
        markdown = format!(
            "{}\n\n[... truncated, {} of {} chars shown]",
            &markdown[..end],
            end,
            markdown.len()
        );
    }

    Ok(FetchHtmlResult {
        content: markdown,
        title: content.title,
        url: url.to_string(),
    })
}

/// Recursively copy a directory and its contents
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
        // Skip symlinks to avoid potential issues
    }
    Ok(())
}

pub(crate) struct ReadableContent {
    pub(crate) content: String,
    pub(crate) title: Option<String>,
}

/// Extract readable content using the readability algorithm
pub(crate) fn extract_readable_content(html: &str, url: &str) -> Result<ReadableContent, String> {
    use readability::extractor;

    // Parse URL for the extractor
    let parsed_url =
        url::Url::parse(url).map_err(|e| format!("Invalid URL for readability: {}", e))?;

    // Create a cursor over the HTML content
    let mut cursor = std::io::Cursor::new(html.as_bytes());

    // Extract readable content
    match extractor::extract(&mut cursor, &parsed_url) {
        Ok(product) => Ok(ReadableContent {
            content: product.content,
            title: if product.title.is_empty() {
                None
            } else {
                Some(product.title)
            },
        }),
        Err(e) => Err(format!("Failed to extract readable content: {}", e)),
    }
}

/// Convert HTML to markdown
pub(crate) fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}
