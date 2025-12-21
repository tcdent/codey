//! Neovim RPC integration
//!
//! Connects to a running Neovim instance via Unix socket and provides
//! methods for displaying diffs, reloading buffers, and sending commands.
//!
//! # Socket Discovery
//!
//! The socket path can be:
//! 1. Explicitly configured via `nvim.socket` in config
//! 2. Auto-discovered from tmux session name: `/tmp/nvim-{session}.sock`
//! 3. Set via `$NVIM_LISTEN_ADDRESS` environment variable

use super::{Ide, IdeEvent, Selection, ToolPreview};
use anyhow::{Context, Result};
use async_trait::async_trait;
use nvim_rs::{compat::tokio::Compat, create::tokio as create, Handler, Neovim, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::WriteHalf;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

/// Type alias for the writer half of the nvim connection
type NvimWriter = Compat<WriteHalf<UnixStream>>;

/// Detect nvim filetype from file path extension
fn detect_filetype(path: &str) -> Option<&'static str> {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| match ext {
            "rs" => Some("rust"),
            "py" => Some("python"),
            "js" => Some("javascript"),
            "ts" => Some("typescript"),
            "tsx" => Some("typescriptreact"),
            "jsx" => Some("javascriptreact"),
            "lua" => Some("lua"),
            "sh" | "bash" => Some("bash"),
            "toml" => Some("toml"),
            "json" => Some("json"),
            "yaml" | "yml" => Some("yaml"),
            "md" => Some("markdown"),
            "html" => Some("html"),
            "css" => Some("css"),
            "sql" => Some("sql"),
            "go" => Some("go"),
            "rb" => Some("ruby"),
            "c" => Some("c"),
            "h" => Some("c"),
            "cpp" | "cc" | "cxx" => Some("cpp"),
            "hpp" | "hxx" => Some("cpp"),
            "java" => Some("java"),
            "kt" => Some("kotlin"),
            "swift" => Some("swift"),
            "zig" => Some("zig"),
            "ex" | "exs" => Some("elixir"),
            "erl" => Some("erlang"),
            "hs" => Some("haskell"),
            "ml" | "mli" => Some("ocaml"),
            "vim" => Some("vim"),
            "dockerfile" => Some("dockerfile"),
            "xml" => Some("xml"),
            "graphql" | "gql" => Some("graphql"),
            "proto" => Some("proto"),
            _ => None,
        })
}

/// Handler for nvim notifications
///
/// Receives notifications from neovim (selection changes, etc.) and
/// forwards them to the app via a channel.
#[derive(Clone)]
struct NvimHandler {
    event_tx: mpsc::Sender<IdeEvent>,
}

impl NvimHandler {
    fn new(event_tx: mpsc::Sender<IdeEvent>) -> Self {
        Self { event_tx }
    }
}

#[async_trait]
impl Handler for NvimHandler {
    type Writer = NvimWriter;

    async fn handle_notify(
        &self,
        name: String,
        args: Vec<Value>,
        _neovim: Neovim<Self::Writer>,
    ) {
        match name.as_str() {
            "codey_selection" => {
                let event = parse_selection_event(&args);
                if let Err(e) = self.event_tx.send(event).await {
                    warn!("Failed to send IDE event: {}", e);
                }
            }
            other => {
                debug!("Ignoring nvim notification: {}", other);
            }
        }
    }
}

/// Parse a selection event from nvim notification args
fn parse_selection_event(args: &[Value]) -> IdeEvent {
    // Expected format: [{ path, content, start_line, end_line }] or [] for cleared
    let selection = args.first().and_then(|v| {
        if v.is_nil() {
            return None;
        }
        let map = v.as_map()?;
        
        let get_str = |key: &str| -> Option<String> {
            map.iter()
                .find(|(k, _)| k.as_str() == Some(key))
                .and_then(|(_, v)| v.as_str().map(|s| s.to_string()))
        };
        let get_u32 = |key: &str| -> Option<u32> {
            map.iter()
                .find(|(k, _)| k.as_str() == Some(key))
                .and_then(|(_, v)| v.as_u64().map(|n| n as u32))
        };

        Some(Selection {
            path: get_str("path")?,
            content: get_str("content").unwrap_or_default(),
            start_line: get_u32("start_line").unwrap_or(0),
            end_line: get_u32("end_line").unwrap_or(0),
        })
    });

    IdeEvent::SelectionChanged(selection)
}

/// Connection to a Neovim instance
pub struct Nvim {
    client: Arc<Mutex<Neovim<NvimWriter>>>,
    socket_path: PathBuf,
    show_diffs: bool,
    auto_reload: bool,
    event_rx: mpsc::Receiver<IdeEvent>,
}

impl Nvim {
    /// Connect to a Neovim instance at the given socket path
    async fn connect(
        socket_path: impl Into<PathBuf>,
        show_diffs: bool,
        auto_reload: bool,
    ) -> Result<Self> {
        let socket_path = socket_path.into();
        info!("Connecting to nvim at {:?}", socket_path);

        // Create channel for IDE events (bounded with small buffer)
        let (event_tx, event_rx) = mpsc::channel(16);
        let handler = NvimHandler::new(event_tx);

        let (client, io_handle) = create::new_path(&socket_path, handler)
            .await
            .with_context(|| format!("Failed to connect to nvim socket: {:?}", socket_path))?;

        // Spawn the IO handler - it runs in the background
        tokio::spawn(async move {
            if let Err(e) = io_handle.await {
                warn!("Nvim IO handler error: {:?}", e);
            }
        });

        // Verify connection by getting nvim version
        let version = client.get_api_info().await;
        debug!("Connected to nvim: {:?}", version.is_ok());

        let nvim = Self {
            client: Arc::new(Mutex::new(client)),
            socket_path,
            show_diffs,
            auto_reload,
            event_rx,
        };

        // Set up autocommands for selection tracking
        nvim.setup_selection_tracking().await?;

        Ok(nvim)
    }

    /// Try to discover and connect to a Neovim instance
    ///
    /// Tries in order:
    /// 1. Explicit socket path if provided
    /// 2. Tmux session-based socket: /tmp/nvim-{session}.sock
    /// 3. $NVIM_LISTEN_ADDRESS environment variable
    pub async fn discover(config: &crate::config::NvimConfig) -> Result<Option<Self>> {
        let show_diffs = config.show_diffs;
        let auto_reload = config.auto_reload;

        // 1. Explicit socket path
        if let Some(path) = &config.socket {
            if path.exists() {
                return Ok(Some(Self::connect(path, show_diffs, auto_reload).await?));
            }
            warn!("Configured nvim socket does not exist: {:?}", path);
        }

        // 2. Tmux session-based socket
        if let Ok(session) = std::env::var("TMUX") {
            // TMUX format: /path/to/socket,pid,session_number
            // We need to get the session name separately
            if !session.is_empty() {
                if let Ok(output) = tokio::process::Command::new("tmux")
                    .args(["display-message", "-p", "#S"])
                    .output()
                    .await
                {
                    let session_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if !session_name.is_empty() {
                        let socket_path = PathBuf::from(format!("/tmp/nvim-{}.sock", session_name));
                        if socket_path.exists() {
                            info!("Discovered nvim socket from tmux session: {:?}", socket_path);
                            return Ok(Some(Self::connect(socket_path, show_diffs, auto_reload).await?));
                        }
                    }
                }
            }
        }

        // 3. Environment variable
        if let Ok(addr) = std::env::var("NVIM_LISTEN_ADDRESS") {
            let path = PathBuf::from(&addr);
            if path.exists() {
                info!("Discovered nvim socket from NVIM_LISTEN_ADDRESS: {:?}", path);
                return Ok(Some(Self::connect(path, show_diffs, auto_reload).await?));
            }
        }

        debug!("No nvim socket discovered");
        Ok(None)
    }

    /// Get the socket path this instance is connected to
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    /// Set up autocommands to track visual mode selection changes
    async fn setup_selection_tracking(&self) -> Result<()> {
        // First, store our channel ID in a global variable
        let channel_info = self.exec_lua("return vim.api.nvim_get_chan_info(0)", vec![]).await?;
        
        let channel_id = channel_info
            .as_map()
            .and_then(|m| m.iter().find(|(k, _)| k.as_str() == Some("id")))
            .and_then(|(_, v)| v.as_i64())
            .unwrap_or(0);
        
        debug!("Neovim assigned channel ID: {}", channel_id);
        
        // Set the channel ID then load the selection tracking script
        let setup_lua = format!(
            "vim.g.codey_channel_id = {}\n{}",
            channel_id,
            include_str!("lua/selection_tracking.lua")
        );
        
        self.exec_lua(&setup_lua, vec![]).await?;
        info!("Set up neovim selection tracking");
        
        Ok(())
    }

    /// Execute a vim command
    pub async fn command(&self, cmd: &str) -> Result<()> {
        let client = self.client.lock().await;
        client
            .command(cmd)
            .await
            .with_context(|| format!("Failed to execute nvim command: {}", cmd))?;
        Ok(())
    }

    /// Execute a lua expression and return the result
    pub async fn exec_lua(&self, code: &str, args: Vec<Value>) -> Result<Value> {
        let client = self.client.lock().await;
        client
            .exec_lua(code, args)
            .await
            .context("Failed to execute lua code")
    }

    /// Reload a buffer by file path (if it's open in nvim) - internal helper
    async fn reload_buffer_internal(&self, file_path: &str) -> Result<()> {
        let args = vec![Value::from(file_path)];
        self.exec_lua(include_str!("lua/reload_buffer.lua"), args).await?;
        Ok(())
    }

    /// Display a side-by-side diff using nvim's built-in diff mode
    async fn show_diff(
        &self,
        original: &str,
        modified: &str,
        title: &str,
        language: Option<&str>,
    ) -> Result<()> {
        debug!("show_diff called for: {}", title);

        let original_lines: Vec<Value> = original.lines().map(|l| Value::from(l)).collect();
        let modified_lines: Vec<Value> = modified.lines().map(|l| Value::from(l)).collect();
        let args = vec![
            Value::from(original_lines),
            Value::from(modified_lines),
            Value::from(title),
            Value::from(language.unwrap_or("")),
        ];

        self.exec_lua(include_str!("lua/show_diff.lua"), args).await?;
        info!("Displayed side-by-side diff in nvim: {}", title);
        Ok(())
    }

    /// Close any open Codey preview buffers - internal helper
    async fn close_diff_buffers(&self) -> Result<()> {
        self.exec_lua(include_str!("lua/close_preview.lua"), vec![]).await?;
        Ok(())
    }

    /// Display file content in a scratch buffer - internal helper
    async fn show_file_preview(
        &self,
        content: &str,
        title: &str,
        language: Option<&str>,
    ) -> Result<()> {
        let lines: Vec<Value> = content.lines().map(|l| Value::from(l)).collect();
        let args = vec![
            Value::from(lines),
            Value::from(title),
            Value::from(language.unwrap_or("")),
        ];

        self.exec_lua(include_str!("lua/show_file_preview.lua"), args).await?;
        info!("Displayed file preview in nvim: {}", title);
        Ok(())
    }
}

// ============================================================================
// Ide Trait Implementation
// ============================================================================

#[async_trait]
impl Ide for Nvim {
    fn name(&self) -> &'static str {
        "neovim"
    }

    async fn show_preview(&self, preview: &ToolPreview) -> Result<()> {
        if !self.show_diffs {
            return Ok(());
        }
        match preview {
            ToolPreview::Diff { path, original, modified } => {
                let lang = detect_filetype(path);
                self.show_diff(original, modified, path, lang).await
            }
            ToolPreview::FileContent { path, content } => {
                let lang = detect_filetype(path);
                self.show_file_preview(content, path, lang).await
            }
        }
    }

    async fn close_preview(&self) -> Result<()> {
        self.close_diff_buffers().await
    }

    async fn reload_buffer(&self, path: &str) -> Result<()> {
        if !self.auto_reload {
            return Ok(());
        }
        self.reload_buffer_internal(path).await
    }

    async fn navigate_to(
        &self,
        path: &str,
        line: Option<u32>,
        column: Option<u32>,
    ) -> Result<()> {
        let args = vec![
            Value::from(path),
            Value::from(line.unwrap_or(1) as i64),
            Value::from(column.unwrap_or(1) as i64),
        ];
        self.exec_lua(include_str!("lua/navigate_to.lua"), args).await?;
        Ok(())
    }

    async fn has_unsaved_changes(&self, path: &str) -> Result<bool> {
        let args = vec![Value::from(path)];
        let result = self.exec_lua(include_str!("lua/has_unsaved_changes.lua"), args).await?;
        Ok(result.as_bool().unwrap_or(false))
    }

    async fn next(&mut self) -> Option<IdeEvent> {
        self.event_rx.recv().await
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NvimConfig;

    #[tokio::test]
    #[ignore] // Requires running nvim instance
    async fn test_discover_nvim() {
        let nvim = Nvim::discover(&NvimConfig::default()).await.unwrap();
        if let Some(nvim) = nvim {
            println!("Connected to {} at {:?}", nvim.name(), nvim.socket_path());
        } else {
            println!("No nvim instance found");
        }
    }

    #[tokio::test]
    #[ignore] // Requires running nvim instance
    async fn test_show_preview() {
        let nvim = Nvim::discover(&NvimConfig::default()).await.unwrap().expect("Need nvim");
        let preview = ToolPreview::Diff {
            path: "test.rs".to_string(),
            original: r#"fn main() {
    println!("hello");
}
"#.to_string(),
            modified: r#"fn main() {
    println!("hello, world!");
    println!("goodbye");
}
"#.to_string(),
        };
        nvim.show_preview(&preview).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires running nvim instance
    async fn test_get_current_file() {
        let nvim = Nvim::discover(&NvimConfig::default()).await.unwrap().expect("Need nvim");
        let file = nvim.get_current_file().await.unwrap();
        println!("Current file: {:?}", file);
    }

    #[tokio::test]
    #[ignore] // Requires running nvim instance
    async fn test_get_selection() {
        let nvim = Nvim::discover(&NvimConfig::default()).await.unwrap().expect("Need nvim");
        let selection = nvim.get_selection().await.unwrap();
        println!("Selection: {:?}", selection);
    }
}
