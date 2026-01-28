//! System I/O operations
//!
//! Pure functions for filesystem, shell, and network operations.
//! These are decoupled from the effect/tool system and use standard types.

use std::fs;
use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

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

/// Default tab width for user display
pub const DEFAULT_TAB_WIDTH: usize = 4;

/// Format text for the agent (LLM)
/// 
/// Converts tabs to literal `\t` escape sequences so the agent can see
/// and correctly reproduce them in edits.
pub fn format_for_agent(text: &str) -> String {
    text.replace('\t', "\\t")
}

/// Format text for user display in the terminal
/// 
/// Expands escaped `\t` sequences to spaces based on tab_width for proper visual alignment.
pub fn format_for_user(text: &str, tab_width: usize) -> String {
    let spaces: String = " ".repeat(tab_width);
    text.replace("\\t", &spaces)
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
            format_for_agent(line),
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
pub async fn fetch_url(url: &str, max_length: Option<usize>) -> Result<FetchResult, String> {
    let max_length = max_length.unwrap_or(50000);

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
                Ok(mut text) => {
                    let original_len = text.len();
                    if text.len() > max_length {
                        // Find a valid UTF-8 boundary at or before max_length
                        let mut end = max_length;
                        while end > 0 && !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text = text[..end].to_string();
                        text.push_str(&format!(
                            "\n\n[... truncated, {} of {} bytes shown]",
                            end, original_len
                        ));
                    }

                    Ok(FetchResult {
                        content: text,
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
