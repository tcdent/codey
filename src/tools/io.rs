//! System I/O operations
//!
//! Pure functions for filesystem, shell, and network operations.
//! These are decoupled from the effect/tool system and use standard types.

use std::fs;
use std::path::Path;
use std::process::Stdio;
#[cfg(feature = "cli")]
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[cfg(feature = "cli")]
use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
#[cfg(feature = "cli")]
use futures::StreamExt;

/// Result of a shell command
#[derive(Debug)]
pub struct ShellResult {
    pub output: String,
    pub exit_code: i32,
    pub success: bool,
}

/// Result of a URL fetch
#[derive(Debug)]
pub struct FetchResult {
    pub content: String,
    pub content_type: String,
    pub size: usize,
}

/// A web search result
#[derive(Debug)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
}

/// Read a file and format with line numbers
///
/// If `start_line` and/or `end_line` are provided, only the specified line range is returned.
/// Line numbers are 1-indexed. Use -1 for `end_line` to read to the end of the file.
/// The output line numbers always reflect the actual line numbers in the file.
pub fn read_file(
    path: &Path,
    start_line: Option<i32>,
    end_line: Option<i32>,
) -> Result<String, String> {
    if !path.exists() {
        return Err(format!("File not found: {}", path.display()));
    }
    if !path.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }

    let content =
        fs::read_to_string(path).map_err(|e| format!("Failed to read file: {}", e))?;

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Convert to 0-indexed range, handling optional bounds
    let start_idx = match start_line {
        Some(s) if s > 0 => (s as usize).saturating_sub(1),
        Some(s) if s < 0 => 0, // Negative start treated as beginning
        _ => 0,
    };

    let end_idx = match end_line {
        Some(e) if e == -1 => total_lines, // -1 means end of file
        Some(e) if e > 0 => (e as usize).min(total_lines),
        Some(e) if e < 0 => total_lines, // Other negative values also mean end
        _ => total_lines,
    };

    // Validate range
    if start_idx >= total_lines {
        return Ok(String::new()); // Start is past end of file
    }

    let end_idx = end_idx.max(start_idx); // Ensure end >= start

    // Calculate line number width based on the highest line number we'll show
    let line_num_width = end_idx.to_string().len().max(4);

    let mut output = String::new();
    for (i, line) in lines.iter().enumerate().skip(start_idx).take(end_idx - start_idx) {
        output.push_str(&format!(
            "{:>width$}│{}\n",
            i + 1, // Line numbers are 1-indexed
            line,
            width = line_num_width
        ));
    }

    Ok(output)
}

/// Execute a shell command
pub async fn execute_shell(
    command: &str,
    working_dir: Option<&str>,
    timeout_secs: u64,
) -> Result<ShellResult, String> {
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(command);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if let Some(dir) = working_dir {
        let path = Path::new(dir);
        if !path.exists() {
            return Err(format!("Working directory does not exist: {}", dir));
        }
        if !path.is_dir() {
            return Err(format!("Not a directory: {}", dir));
        }
        cmd.current_dir(dir);
    }

    let mut child = cmd.spawn().map_err(|e| format!("Failed to spawn: {}", e))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let mut collected = String::new();

    if let Some(stdout) = stdout {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            collected.push_str(&line);
            collected.push('\n');
        }
    }

    let mut stderr_output = String::new();
    if let Some(stderr) = stderr {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stderr_output.push_str(&line);
            stderr_output.push('\n');
        }
    }

    let status = match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait(),
    )
    .await
    {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("Wait failed: {}", e)),
        Err(_) => {
            let _ = child.kill().await;
            return Err(format!(
                "Command timed out after {} seconds",
                timeout_secs
            ));
        }
    };

    let exit_code = status.code().unwrap_or(-1);
    let mut output = collected;

    if !stderr_output.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("[stderr]\n");
        output.push_str(&stderr_output);
    }

    if output.is_empty() {
        output = "(no output)".to_string();
    }

    if exit_code != 0 {
        output.push_str(&format!("\n[exit code: {}]", exit_code));
    }

    // Truncate if too long (UTF-8 safe)
    const MAX_OUTPUT: usize = 50000;
    if output.len() > MAX_OUTPUT {
        // Find a valid UTF-8 boundary at or before MAX_OUTPUT
        let mut end = MAX_OUTPUT;
        while end > 0 && !output.is_char_boundary(end) {
            end -= 1;
        }
        output = format!(
            "{}\n\n[... output truncated ({} bytes total)]",
            &output[..end],
            output.len()
        );
    }

    Ok(ShellResult {
        output,
        exit_code,
        success: status.success(),
    })
}

/// Fetch content from a URL
///
/// For HTML content, automatically converts to markdown for more efficient
/// token usage. Scripts, styles, and other noise are stripped.
pub async fn fetch_url(url: &str, max_length: Option<usize>) -> Result<FetchResult, String> {
    let max_length = max_length.unwrap_or(20000);

    let parsed_url =
        url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;

    if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
        return Err(format!(
            "Unsupported URL scheme: {}. Only http and https are allowed.",
            parsed_url.scheme()
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client.get(url).send(),
    )
    .await;

    match result {
        Ok(Ok(response)) => {
            let status = response.status();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();

            if !status.is_success() {
                return Err(format!(
                    "HTTP error: {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ));
            }

            match response.text().await {
                Ok(text) => {
                    let original_len = text.len();

                    // Convert HTML to markdown for more efficient token usage
                    let is_html = content_type.contains("text/html")
                        || text.trim_start().starts_with("<!DOCTYPE")
                        || text.trim_start().starts_with("<html");

                    let mut processed = if is_html {
                        let cleaned = strip_html_noise(&text);
                        htmd::convert(&cleaned).unwrap_or(cleaned)
                    } else {
                        text
                    };

                    let processed_len = processed.len();

                    // Truncate if needed
                    if processed.len() > max_length {
                        let mut end = max_length;
                        while end > 0 && !processed.is_char_boundary(end) {
                            end -= 1;
                        }
                        processed = processed[..end].to_string();
                        processed.push_str(&format!(
                            "\n\n[... truncated, {} of {} chars shown (original: {} bytes)]",
                            end, processed_len, original_len
                        ));
                    }

                    Ok(FetchResult {
                        content: processed,
                        content_type,
                        size: original_len,
                    })
                }
                Err(e) => Err(format!("Failed to read response body: {}", e)),
            }
        }
        Ok(Err(e)) => Err(format!("Request failed: {}", e)),
        Err(_) => Err("Request timed out after 30 seconds".to_string()),
    }
}

/// Strip noise elements from HTML (scripts, styles, nav, etc.)
fn strip_html_noise(html: &str) -> String {
    use fancy_regex::Regex;

    // Remove script tags and their content
    let script_re = Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let html = script_re.replace_all(html, "");

    // Remove style tags and their content
    let style_re = Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap();
    let html = style_re.replace_all(&html, "");

    // Remove common noise elements (keep content simple - just remove the tags)
    let noise_re = Regex::new(r"(?is)<(nav|header|footer|aside|noscript)[^>]*>.*?</\1>").unwrap();
    let html = noise_re.replace_all(&html, "");

    // Remove HTML comments
    let comment_re = Regex::new(r"(?s)<!--.*?-->").unwrap();
    let html = comment_re.replace_all(&html, "");

    // Remove SVG elements (often large and unhelpful as text)
    let svg_re = Regex::new(r"(?is)<svg[^>]*>.*?</svg>").unwrap();
    let html = svg_re.replace_all(&html, "");

    html.to_string()
}

/// Search the web using Brave Search API
pub async fn web_search(query: &str, count: u32) -> Result<Vec<SearchResult>, String> {
    let api_key = std::env::var("BRAVE_API_KEY").map_err(|_| {
        "BRAVE_API_KEY environment variable not set. \
         Get an API key from https://brave.com/search/api/"
            .to_string()
    })?;

    let count = count.min(20);
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
        urlencoding::encode(query),
        count
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("Codey/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &api_key)
            .send(),
    )
    .await;

    match result {
        Ok(Ok(response)) => {
            let status = response.status();
            if !status.is_success() {
                let error_text = response.text().await.unwrap_or_default();
                return Err(format!(
                    "Brave Search API error: {} {} - {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown"),
                    error_text
                ));
            }

            match response.json::<BraveSearchResponse>().await {
                Ok(search_response) => {
                    let results = search_response
                        .web
                        .map(|w| {
                            w.results
                                .into_iter()
                                .map(|r| SearchResult {
                                    title: r.title,
                                    url: r.url,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Ok(results)
                }
                Err(e) => Err(format!("Failed to parse Brave Search response: {}", e)),
            }
        }
        Ok(Err(e)) => Err(format!("Request failed: {}", e)),
        Err(_) => Err("Request timed out after 30 seconds".to_string()),
    }
}

// Brave Search API response structures
#[derive(Debug, serde::Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<WebResults>,
}

#[derive(Debug, serde::Deserialize)]
struct WebResults {
    #[serde(default)]
    results: Vec<WebResult>,
}

#[derive(Debug, serde::Deserialize)]
struct WebResult {
    title: String,
    url: String,
}

/// Result of fetching HTML content as readable markdown
#[cfg(feature = "cli")]
#[derive(Debug)]
pub struct FetchHtmlResult {
    pub content: String,
    pub title: Option<String>,
    pub url: String,
}

/// Detect available browser executable for headless rendering
#[cfg(feature = "cli")]
pub fn detect_browser() -> Option<String> {
    // Common browser executable names in order of preference
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
#[cfg(feature = "cli")]
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
#[cfg(feature = "cli")]
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

    // Require browser
    let browser_path = detect_browser().ok_or_else(|| {
        "No Chrome/Chromium browser found. Install chromium or google-chrome to use this tool."
            .to_string()
    })?;

    let html = fetch_with_browser(url, Some(&browser_path)).await?;

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
#[cfg(feature = "cli")]
async fn fetch_with_browser(url: &str, browser_path: Option<&str>) -> Result<String, String> {
    const JS_RENDER_WAIT_MS: u64 = 2000;
    // Configure browser
    let mut config = BrowserConfig::builder()
        .no_sandbox()
        .arg("--disable-gpu")
        .headless_mode(HeadlessMode::True);

    if let Some(path) = browser_path {
        config = config.chrome_executable(path);
    }

    let config = config.build().map_err(|e| format!("Failed to configure browser: {:?}", e))?;

    // Launch browser with timeout
    let launch_result = tokio::time::timeout(
        Duration::from_secs(30),
        Browser::launch(config),
    )
    .await;

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
    let page_result = tokio::time::timeout(
        Duration::from_secs(60),
        async {
            let page = browser.new_page(url)
                .await
                .map_err(|e| format!("Failed to navigate: {}", e))?;

            // Wait for JavaScript to render
            tokio::time::sleep(Duration::from_millis(JS_RENDER_WAIT_MS)).await;

            // Get rendered HTML
            page.content()
                .await
                .map_err(|e| format!("Failed to get page content: {}", e))
        },
    )
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

#[cfg(feature = "cli")]
struct ReadableContent {
    content: String,
    title: Option<String>,
}

/// Extract readable content using the readability algorithm
#[cfg(feature = "cli")]
fn extract_readable_content(html: &str, url: &str) -> Result<ReadableContent, String> {
    use readability::extractor;

    // Parse URL for the extractor
    let parsed_url = url::Url::parse(url)
        .map_err(|e| format!("Invalid URL for readability: {}", e))?;

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
#[cfg(feature = "cli")]
fn html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| html.to_string())
}
