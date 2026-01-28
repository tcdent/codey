//! Browser automation module
//!
//! Provides headless browser functionality for fetching JavaScript-rendered pages.
//! Uses Chrome/Chromium via the chromiumoxide crate.
//!
//! # Features
//!
//! - Headless browser page rendering with JavaScript support
//! - Chrome profile support for authenticated sessions
//! - Readability-based content extraction
//! - HTML to Markdown conversion for token-efficient output
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
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::handler::viewport::Viewport;
use futures::StreamExt;

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
/// This function:
/// 1. Requires Chrome/Chromium to be installed
/// 2. Renders page with headless browser (handles SPAs/JS)
/// 3. Applies readability algorithm to extract main content
/// 4. Converts to markdown for token-efficient representation
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

    // Get browser settings from global context
    let ctx = browser_context();

    // Resolve browser executable: context -> auto-detect
    let browser_path = ctx
        .and_then(|c| c.chrome_executable.clone())
        .or_else(detect_browser)
        .ok_or_else(|| {
            "No Chrome/Chromium browser found. Install chromium or google-chrome to use this tool."
                .to_string()
        })?;

    // Get settings from context
    let ctx = ctx.cloned().unwrap_or_default();

    let html = fetch_with_browser(url, &browser_path, &ctx).await?;

    // Apply readability to extract main content
    let readable = extract_readable_content(&html, url)?;

    // Convert to markdown
    let mut markdown = html_to_markdown(&readable.content);

    // Truncate if needed
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
        title: readable.title,
        url: url.to_string(),
    })
}

/// Fetch page using headless browser (handles JavaScript rendering)
///
/// # Arguments
/// * `url` - The URL to fetch
/// * `browser_path` - Path to Chrome/Chromium executable
/// * `ctx` - Browser context with profile, viewport, and timing settings
async fn fetch_with_browser(
    url: &str,
    browser_path: &str,
    ctx: &BrowserContext,
) -> Result<String, String> {
    // Copy profile to isolated temp directory to avoid SingletonLock conflicts.
    // See module doc comment for full explanation of why this is necessary.
    let temp_dir = if let (Some(data_dir), Some(prof)) =
        (&ctx.chrome_user_data_dir, &ctx.chrome_profile)
    {
        let source_profile = std::path::Path::new(data_dir).join(prof);
        if source_profile.exists() {
            // Use PID to isolate temp dirs between concurrent Codey instances
            let temp_base =
                std::env::temp_dir().join(format!("codey-browser-{}", std::process::id()));
            // Clean up any previous temp dir to ensure fresh copy
            if temp_base.exists() {
                let _ = std::fs::remove_dir_all(&temp_base);
            }
            std::fs::create_dir_all(&temp_base)
                .map_err(|e| format!("Failed to create temp browser dir: {}", e))?;

            // Copy profile to temp_base/Default (becomes the default profile)
            let dest_profile = temp_base.join("Default");
            copy_dir_recursive(&source_profile, &dest_profile)
                .map_err(|e| format!("Failed to copy browser profile: {}", e))?;

            Some(temp_base)
        } else {
            return Err(format!(
                "Browser profile not found: {}",
                source_profile.display()
            ));
        }
    } else {
        None
    };

    // Configure browser
    let headless_mode = if ctx.headless {
        HeadlessMode::True
    } else {
        HeadlessMode::False
    };

    // When using a copied profile, we need real Keychain access to decrypt cookies.
    //
    // chromiumoxide's DEFAULT_ARGS (from Puppeteer) include:
    //   "--password-store=basic"  - Uses basic password store instead of OS keychain
    //   "--use-mock-keychain"     - Uses mock keychain for testing
    //
    // These flags are designed for automation/testing where you don't want system prompts,
    // but they prevent Chrome from decrypting cookies that were encrypted with the real
    // Keychain key. We must disable defaults and provide our own args list.
    //
    // Reference: chromiumoxide 0.7.0 browser.rs DEFAULT_ARGS
    // Original source: https://github.com/nickelc/chromiumoxide/blob/v0.7.0/src/browser.rs
    let using_profile = temp_dir.is_some();

    let mut config = BrowserConfig::builder()
        .no_sandbox()
        .headless_mode(headless_mode)
        .window_size(ctx.viewport_width, ctx.viewport_height)
        .viewport(Viewport {
            width: ctx.viewport_width,
            height: ctx.viewport_height,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        });

    if using_profile {
        // Disable chromiumoxide defaults and add our own (without mock keychain flags)
        config = config.disable_default_args().args([
            "--disable-background-networking",
            "--enable-features=NetworkService,NetworkServiceInProcess",
            "--disable-background-timer-throttling",
            "--disable-backgrounding-occluded-windows",
            "--disable-breakpad",
            "--disable-client-side-phishing-detection",
            "--disable-component-extensions-with-background-pages",
            "--disable-default-apps",
            "--disable-dev-shm-usage",
            "--disable-extensions",
            "--disable-features=TranslateUI",
            "--disable-hang-monitor",
            "--disable-ipc-flooding-protection",
            "--disable-popup-blocking",
            "--disable-prompt-on-repost",
            "--disable-renderer-backgrounding",
            "--disable-sync",
            "--force-color-profile=srgb",
            "--metrics-recording-only",
            "--no-first-run",
            "--enable-automation",
            // OMITTED: "--password-store=basic" - need real password store for cookies
            // OMITTED: "--use-mock-keychain" - need real Keychain access
            "--enable-blink-features=IdleDetection",
            "--lang=en_US",
            "--disable-gpu",
        ]);
    } else {
        config = config.arg("--disable-gpu");
    }

    config = config.chrome_executable(browser_path);

    // Use temp dir if we copied a profile, otherwise use user_data_dir directly
    if let Some(ref temp) = temp_dir {
        config = config.user_data_dir(temp);
    } else if let Some(ref dir) = ctx.chrome_user_data_dir {
        // Fallback: use user_data_dir directly (may conflict with running Chrome)
        config = config.user_data_dir(dir);
        if let Some(ref prof) = ctx.chrome_profile {
            config = config.arg(format!("--profile-directory={}", prof));
        }
    }

    let config = config
        .build()
        .map_err(|e| format!("Failed to configure browser: {:?}", e))?;

    // Launch browser with timeout
    let launch_result =
        tokio::time::timeout(Duration::from_secs(30), Browser::launch(config)).await;

    let (mut browser, mut handler) = match launch_result {
        Ok(Ok((browser, handler))) => (browser, handler),
        Ok(Err(e)) => return Err(format!("Failed to launch browser: {}", e)),
        Err(_) => return Err("Browser launch timed out after 30 seconds".to_string()),
    };

    // Spawn handler task (required by chromiumoxide)
    let handle = tokio::spawn(async move {
        while let Some(_event) = handler.next().await {
            // Process browser events
        }
    });

    // Navigate to page
    let page_result = tokio::time::timeout(Duration::from_secs(60), async {
        let page = browser
            .new_page(url)
            .await
            .map_err(|e| format!("Failed to navigate: {}", e))?;

        // Wait for JavaScript to render (configurable for SPAs like Twitter)
        tokio::time::sleep(Duration::from_millis(ctx.page_load_wait_ms)).await;

        // Get rendered HTML
        page.content()
            .await
            .map_err(|e| format!("Failed to get page content: {}", e))
    })
    .await;

    // Clean up
    let _ = browser.close().await;
    handle.abort();

    match page_result {
        Ok(Ok(html)) => Ok(html),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Page load timed out after 60 seconds".to_string()),
    }
}

/// Recursively copy a directory and its contents
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
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

struct ReadableContent {
    content: String,
    title: Option<String>,
}

/// Extract readable content using the readability algorithm
fn extract_readable_content(html: &str, url: &str) -> Result<ReadableContent, String> {
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
fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}
