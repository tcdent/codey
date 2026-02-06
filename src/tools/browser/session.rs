//! Browser session management
//!
//! Persistent browser sessions that agents can open, interact with, and close.
//! Sessions are identified by name and hold onto a chromiumoxide Browser + Page
//! handle so the browser process stays alive between tool calls.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::page::{NavigateParams, ReloadParams};
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::Page;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::{browser_context, copy_dir_recursive, detect_browser, BrowserContext};

/// Maximum idle time before a session is automatically closed (10 minutes)
const SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// How long to wait for page load before timing out
const PAGE_LOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// How long to wait after network activity stops to consider page "settled"
const NETWORK_SETTLE_MS: u64 = 1000;

/// A single browser session with its browser and page handles
struct BrowserSession {
    browser: Browser,
    page: Page,
    /// Background task running the chromiumoxide event handler
    _handler_task: JoinHandle<()>,
    /// Temp directory for profile copy (cleaned up on drop)
    _temp_dir: Option<std::path::PathBuf>,
    /// Current URL
    url: String,
    /// Last time this session was accessed
    last_accessed: Instant,
}

impl BrowserSession {
    fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }

    fn is_expired(&self) -> bool {
        self.last_accessed.elapsed() > SESSION_IDLE_TIMEOUT
    }
}

/// Actions the agent can perform on a browser session
#[derive(Debug, Clone)]
pub enum BrowserAction {
    /// Navigate to a new URL
    Navigate { url: String },
    /// Click an element by CSS selector
    Click { selector: String },
    /// Type into an input element
    Fill { selector: String, value: String },
    /// Select an option from a dropdown
    Select { selector: String, value: String },
    /// Scroll the page
    Scroll { direction: ScrollDirection, amount: Option<u32> },
    /// Go back in history
    Back,
    /// Go forward in history
    Forward,
    /// Wait for a specified duration
    Wait { ms: u64 },
    /// Execute JavaScript and return result
    Evaluate { script: String },
}

impl BrowserAction {
    /// Parse an action name + JSON params into a BrowserAction.
    ///
    /// This keeps all the action-specific validation in one place,
    /// so callers just pass the raw strings from the tool call.
    pub fn parse(action: &str, params: &serde_json::Value) -> Result<Self, String> {
        match action {
            "navigate" => {
                let url = params["url"].as_str()
                    .ok_or("navigate action requires 'url' parameter")?;
                Ok(BrowserAction::Navigate { url: url.to_string() })
            }
            "click" => {
                let selector = params["selector"].as_str()
                    .ok_or("click action requires 'selector' parameter")?;
                Ok(BrowserAction::Click { selector: selector.to_string() })
            }
            "fill" => {
                let selector = params["selector"].as_str()
                    .ok_or("fill action requires 'selector' parameter")?;
                let value = params["value"].as_str()
                    .ok_or("fill action requires 'value' parameter")?;
                Ok(BrowserAction::Fill {
                    selector: selector.to_string(),
                    value: value.to_string(),
                })
            }
            "select" => {
                let selector = params["selector"].as_str()
                    .ok_or("select action requires 'selector' parameter")?;
                let value = params["value"].as_str()
                    .ok_or("select action requires 'value' parameter")?;
                Ok(BrowserAction::Select {
                    selector: selector.to_string(),
                    value: value.to_string(),
                })
            }
            "scroll" => {
                let direction = match params["direction"].as_str().unwrap_or("down") {
                    "up" => ScrollDirection::Up,
                    _ => ScrollDirection::Down,
                };
                let amount = params["amount"].as_u64().map(|v| v as u32);
                Ok(BrowserAction::Scroll { direction, amount })
            }
            "back" => Ok(BrowserAction::Back),
            "forward" => Ok(BrowserAction::Forward),
            "wait" => {
                let ms = params["ms"].as_u64().unwrap_or(1000);
                Ok(BrowserAction::Wait { ms })
            }
            "evaluate" => {
                let script = params["script"].as_str()
                    .ok_or("evaluate action requires 'script' parameter")?;
                Ok(BrowserAction::Evaluate { script: script.to_string() })
            }
            other => Err(format!(
                "Unknown action '{}'. Valid actions: navigate, click, fill, select, \
                 scroll, back, forward, wait, evaluate",
                other
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ScrollDirection {
    Up,
    Down,
}

/// Result of a browser session operation
#[derive(Debug)]
pub struct SessionResult {
    pub session_name: String,
    pub url: String,
    pub title: Option<String>,
    pub content: String,
}

impl SessionResult {
    /// Format the result as a string for the LLM.
    pub fn format(&self) -> String {
        let title_info = self.title
            .as_ref()
            .map(|t| format!("[Title: {}]\n", t))
            .unwrap_or_default();
        format!(
            "[Session: {}]\n[URL: {}]\n{}\n{}",
            self.session_name, self.url, title_info, self.content
        )
    }
}

/// Session info for listing
#[derive(Debug)]
pub struct SessionInfo {
    pub name: String,
    pub url: String,
    pub idle_secs: u64,
}

/// Manages named browser sessions
///
/// Thread-safe via Arc<Mutex<>> — sessions are accessed from async effect
/// handlers that may run concurrently.
pub struct BrowserSessionManager {
    sessions: Arc<Mutex<HashMap<String, BrowserSession>>>,
}

impl BrowserSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Open a new browser session or reuse an existing one.
    ///
    /// Launches a browser, navigates to the URL, waits for page load,
    /// and returns the readability-extracted content.
    pub async fn open(
        &self,
        url: &str,
        session_name: Option<String>,
    ) -> Result<SessionResult, String> {
        let name = session_name.unwrap_or_else(|| generate_session_name());

        // Check if session already exists
        {
            let mut sessions = self.sessions.lock().await;
            if let Some(session) = sessions.get_mut(&name) {
                // Session exists — navigate to the new URL
                session.touch();
                navigate_and_wait(&session.page, url).await?;
                session.url = url.to_string();
                let content = extract_page_content(&session.page, url).await?;
                return Ok(SessionResult {
                    session_name: name,
                    url: url.to_string(),
                    title: content.title,
                    content: content.markdown,
                });
            }
        }

        // Clean up expired sessions
        self.cleanup_expired().await;

        // Launch new browser session
        let (browser, page, handler_task, temp_dir) = launch_browser(url).await?;

        let content = extract_page_content(&page, url).await?;

        let session = BrowserSession {
            browser,
            page,
            _handler_task: handler_task,
            _temp_dir: temp_dir,
            url: url.to_string(),
            last_accessed: Instant::now(),
        };

        let result = SessionResult {
            session_name: name.clone(),
            url: url.to_string(),
            title: content.title,
            content: content.markdown,
        };

        self.sessions.lock().await.insert(name, session);

        Ok(result)
    }

    /// Perform an action on an existing session.
    pub async fn action(
        &self,
        session_name: &str,
        action: BrowserAction,
    ) -> Result<SessionResult, String> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(session_name)
            .ok_or_else(|| format!("Session '{}' not found. Use browser_open to create one.", session_name))?;

        session.touch();

        match action {
            BrowserAction::Navigate { url } => {
                navigate_and_wait(&session.page, &url).await?;
                session.url = url;
            }
            BrowserAction::Click { selector } => {
                session
                    .page
                    .find_element(&selector)
                    .await
                    .map_err(|e| format!("Element not found '{}': {}", selector, e))?
                    .click()
                    .await
                    .map_err(|e| format!("Click failed on '{}': {}", selector, e))?;
                wait_for_settle(&session.page).await;
            }
            BrowserAction::Fill { selector, value } => {
                session
                    .page
                    .find_element(&selector)
                    .await
                    .map_err(|e| format!("Element not found '{}': {}", selector, e))?
                    .click()
                    .await
                    .map_err(|e| format!("Focus failed on '{}': {}", selector, e))?;
                session
                    .page
                    .find_element(&selector)
                    .await
                    .map_err(|e| format!("Element not found '{}': {}", selector, e))?
                    .type_str(&value)
                    .await
                    .map_err(|e| format!("Fill failed on '{}': {}", selector, e))?;
                wait_for_settle(&session.page).await;
            }
            BrowserAction::Select { selector, value } => {
                // Use JavaScript to set select value
                let script = format!(
                    r#"document.querySelector('{}').value = '{}';
                       document.querySelector('{}').dispatchEvent(new Event('change', {{ bubbles: true }}))"#,
                    selector.replace('\'', "\\'"),
                    value.replace('\'', "\\'"),
                    selector.replace('\'', "\\'"),
                );
                session
                    .page
                    .evaluate(script)
                    .await
                    .map_err(|e| format!("Select failed on '{}': {}", selector, e))?;
                wait_for_settle(&session.page).await;
            }
            BrowserAction::Scroll { direction, amount } => {
                let pixels = amount.unwrap_or(500) as i32;
                let delta = match direction {
                    ScrollDirection::Up => -pixels,
                    ScrollDirection::Down => pixels,
                };
                let script = format!("window.scrollBy(0, {})", delta);
                session
                    .page
                    .evaluate(script)
                    .await
                    .map_err(|e| format!("Scroll failed: {}", e))?;
                // Short wait for any lazy-loaded content
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            BrowserAction::Back => {
                let script = "window.history.back()";
                session
                    .page
                    .evaluate(script)
                    .await
                    .map_err(|e| format!("Back navigation failed: {}", e))?;
                wait_for_settle(&session.page).await;
            }
            BrowserAction::Forward => {
                let script = "window.history.forward()";
                session
                    .page
                    .evaluate(script)
                    .await
                    .map_err(|e| format!("Forward navigation failed: {}", e))?;
                wait_for_settle(&session.page).await;
            }
            BrowserAction::Wait { ms } => {
                let capped = ms.min(30000); // Cap at 30s
                tokio::time::sleep(Duration::from_millis(capped)).await;
            }
            BrowserAction::Evaluate { script } => {
                let result = session
                    .page
                    .evaluate(script.as_str())
                    .await
                    .map_err(|e| format!("JavaScript evaluation failed: {}", e))?;
                // For evaluate, return the JS result directly instead of page content
                let js_result = result
                    .value()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "undefined".to_string());
                let current_url = get_current_url(&session.page).await.unwrap_or(session.url.clone());
                session.url = current_url.clone();
                return Ok(SessionResult {
                    session_name: session_name.to_string(),
                    url: current_url,
                    title: None,
                    content: format!("[JavaScript Result]\n{}", js_result),
                });
            }
        }

        // Update URL after action (navigation may have changed it)
        let current_url = get_current_url(&session.page)
            .await
            .unwrap_or(session.url.clone());
        session.url = current_url.clone();

        let content = extract_page_content(&session.page, &current_url).await?;

        Ok(SessionResult {
            session_name: session_name.to_string(),
            url: current_url,
            title: content.title,
            content: content.markdown,
        })
    }

    /// Get a snapshot of the current page content.
    pub async fn snapshot(
        &self,
        session_name: &str,
    ) -> Result<SessionResult, String> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(session_name)
            .ok_or_else(|| format!("Session '{}' not found", session_name))?;

        session.touch();

        let content = extract_page_content(&session.page, &session.url).await?;

        Ok(SessionResult {
            session_name: session_name.to_string(),
            url: session.url.clone(),
            title: content.title,
            content: content.markdown,
        })
    }

    /// Perform an action on a session from raw action name + JSON params.
    ///
    /// This is the high-level API for effect handlers — parses the action
    /// string, executes it, and returns a formatted result string.
    pub async fn action_from_raw(
        &self,
        session_name: &str,
        action: &str,
        params: &serde_json::Value,
    ) -> Result<String, String> {
        let browser_action = BrowserAction::parse(action, params)?;
        let result = self.action(session_name, browser_action).await?;
        Ok(result.format())
    }

    /// List all active sessions.
    pub async fn list(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.lock().await;
        sessions
            .iter()
            .map(|(name, session)| SessionInfo {
                name: name.clone(),
                url: session.url.clone(),
                idle_secs: session.last_accessed.elapsed().as_secs(),
            })
            .collect()
    }

    /// List all active sessions as a formatted string.
    pub async fn format_list(&self) -> String {
        let sessions = self.list().await;
        if sessions.is_empty() {
            "No active browser sessions".to_string()
        } else {
            sessions
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. {} [{}] idle {}s", i + 1, s.name, s.url, s.idle_secs))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    /// Close a specific session.
    pub async fn close(&self, session_name: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().await;
        if let Some(mut session) = sessions.remove(session_name) {
            let _ = session.browser.close().await;
            session._handler_task.abort();
            if let Some(ref temp) = session._temp_dir {
                let _ = std::fs::remove_dir_all(temp);
            }
            Ok(())
        } else {
            Err(format!("Session '{}' not found", session_name))
        }
    }

    /// Close all sessions (called on app shutdown).
    pub async fn close_all(&self) {
        let mut sessions = self.sessions.lock().await;
        for (_, mut session) in sessions.drain() {
            let _ = session.browser.close().await;
            session._handler_task.abort();
            if let Some(ref temp) = session._temp_dir {
                let _ = std::fs::remove_dir_all(temp);
            }
        }
    }

    /// Remove expired sessions.
    async fn cleanup_expired(&self) {
        let mut sessions = self.sessions.lock().await;
        let expired: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.is_expired())
            .map(|(name, _)| name.clone())
            .collect();

        for name in expired {
            if let Some(mut session) = sessions.remove(&name) {
                let _ = session.browser.close().await;
                session._handler_task.abort();
                if let Some(ref temp) = session._temp_dir {
                    let _ = std::fs::remove_dir_all(temp);
                }
                tracing::info!("Closed expired browser session '{}'", name);
            }
        }
    }
}

// =============================================================================
// Internal helpers
// =============================================================================

/// Extracted page content
pub(crate) struct PageContent {
    pub(crate) title: Option<String>,
    pub(crate) markdown: String,
}

/// Launch a new browser instance and navigate to the given URL.
pub(crate) async fn launch_browser(
    url: &str,
) -> Result<(Browser, Page, JoinHandle<()>, Option<std::path::PathBuf>), String> {
    let ctx = browser_context().cloned().unwrap_or_default();

    let browser_path = ctx
        .chrome_executable
        .clone()
        .or_else(detect_browser)
        .ok_or_else(|| {
            "No Chrome/Chromium browser found. Install chromium or google-chrome.".to_string()
        })?;

    // Copy profile if configured (same logic as fetch_with_browser)
    let temp_dir = if let (Some(data_dir), Some(prof)) =
        (&ctx.chrome_user_data_dir, &ctx.chrome_profile)
    {
        let source_profile = std::path::Path::new(data_dir).join(prof);
        if source_profile.exists() {
            let temp_base = std::env::temp_dir().join(format!(
                "codey-browser-session-{}",
                std::process::id()
            ));
            if temp_base.exists() {
                let _ = std::fs::remove_dir_all(&temp_base);
            }
            std::fs::create_dir_all(&temp_base)
                .map_err(|e| format!("Failed to create temp browser dir: {}", e))?;
            let dest_profile = temp_base.join("Default");
            copy_dir_recursive(&source_profile, &dest_profile)
                .map_err(|e| format!("Failed to copy browser profile: {}", e))?;
            Some(temp_base)
        } else {
            None
        }
    } else {
        None
    };

    let using_profile = temp_dir.is_some();

    let headless_mode = if ctx.headless {
        HeadlessMode::True
    } else {
        HeadlessMode::False
    };

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
            "--enable-blink-features=IdleDetection",
            "--lang=en_US",
            "--disable-gpu",
        ]);
    } else {
        config = config.arg("--disable-gpu");
    }

    config = config.chrome_executable(&browser_path);

    if let Some(ref temp) = temp_dir {
        config = config.user_data_dir(temp);
    } else if let Some(ref dir) = ctx.chrome_user_data_dir {
        config = config.user_data_dir(dir);
        if let Some(ref prof) = ctx.chrome_profile {
            config = config.arg(format!("--profile-directory={}", prof));
        }
    }

    let config = config
        .build()
        .map_err(|e| format!("Failed to configure browser: {:?}", e))?;

    // Launch browser
    let launch_result =
        tokio::time::timeout(Duration::from_secs(30), Browser::launch(config)).await;

    let (browser, mut handler) = match launch_result {
        Ok(Ok((browser, handler))) => (browser, handler),
        Ok(Err(e)) => return Err(format!("Failed to launch browser: {}", e)),
        Err(_) => return Err("Browser launch timed out after 30 seconds".to_string()),
    };

    // Spawn handler task
    let handler_task = tokio::spawn(async move {
        while let Some(_event) = handler.next().await {
            // Process browser events
        }
    });

    // Navigate to initial URL
    let page = tokio::time::timeout(PAGE_LOAD_TIMEOUT, async {
        browser
            .new_page(url)
            .await
            .map_err(|e| format!("Failed to navigate to {}: {}", url, e))
    })
    .await
    .map_err(|_| "Page load timed out".to_string())??;

    // Wait for page to settle
    wait_for_settle(&page).await;

    Ok((browser, page, handler_task, temp_dir))
}

/// Navigate to a URL on an existing page and wait for it to settle.
async fn navigate_and_wait(page: &Page, url: &str) -> Result<(), String> {
    tokio::time::timeout(PAGE_LOAD_TIMEOUT, async {
        page.execute(NavigateParams::new(url))
            .await
            .map_err(|e| format!("Navigation failed: {}", e))?;
        wait_for_settle(page).await;
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| format!("Navigation to {} timed out", url))?
}

/// Wait for the page to settle after an action.
///
/// Uses a simple heuristic: wait for network activity to quiet down.
/// We poll the document readyState and wait for a short debounce period
/// with no changes.
async fn wait_for_settle(page: &Page) {
    // Wait for document.readyState to be at least "interactive"
    for _ in 0..30 {
        let ready = page
            .evaluate("document.readyState")
            .await
            .ok()
            .and_then(|v| v.into_value::<String>().ok());

        match ready.as_deref() {
            Some("complete") => break,
            Some("interactive") => {
                // DOM is ready, give a bit more time for async content
                tokio::time::sleep(Duration::from_millis(200)).await;
                break;
            }
            _ => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }

    // Additional settle time for SPA content loading
    tokio::time::sleep(Duration::from_millis(NETWORK_SETTLE_MS)).await;
}

/// Get the current URL from the page.
async fn get_current_url(page: &Page) -> Option<String> {
    page.evaluate("window.location.href")
        .await
        .ok()
        .and_then(|v| v.into_value::<String>().ok())
}

/// Extract readable content from the current page.
pub(crate) async fn extract_page_content(page: &Page, url: &str) -> Result<PageContent, String> {
    let html = page
        .content()
        .await
        .map_err(|e| format!("Failed to get page content: {}", e))?;

    let readable = super::extract_readable_content(&html, url)?;
    let markdown = super::html_to_markdown(&readable.content);

    Ok(PageContent {
        title: readable.title,
        markdown,
    })
}

/// Generate a short session name from a counter.
fn generate_session_name() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    format!("session-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}
