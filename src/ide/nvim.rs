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

use super::{CurrentFile, Ide, NotifyLevel, Selection, ToolPreview, VisibleRange};
use anyhow::{Context, Result};
use async_trait::async_trait;
use nvim_rs::{compat::tokio::Compat, create::tokio as create, Handler, Neovim, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::WriteHalf;
use tokio::net::UnixStream;
use tokio::sync::Mutex;
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

/// Dummy handler for nvim notifications/requests (we don't need to handle any)
#[derive(Clone)]
struct NvimHandler;

impl Handler for NvimHandler {
    type Writer = NvimWriter;
}

/// Connection to a Neovim instance
pub struct Nvim {
    client: Arc<Mutex<Neovim<NvimWriter>>>,
    socket_path: PathBuf,
}

impl Nvim {
    /// Connect to a Neovim instance at the given socket path
    pub async fn connect(socket_path: impl Into<PathBuf>) -> Result<Self> {
        let socket_path = socket_path.into();
        info!("Connecting to nvim at {:?}", socket_path);

        let (client, io_handle) = create::new_path(&socket_path, NvimHandler)
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

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            socket_path,
        })
    }

    /// Try to discover and connect to a Neovim instance
    ///
    /// Tries in order:
    /// 1. Explicit socket path if provided
    /// 2. Tmux session-based socket: /tmp/nvim-{session}.sock
    /// 3. $NVIM_LISTEN_ADDRESS environment variable
    pub async fn discover(explicit_socket: Option<PathBuf>) -> Result<Option<Self>> {
        // 1. Explicit socket path
        if let Some(path) = explicit_socket {
            if path.exists() {
                return Ok(Some(Self::connect(path).await?));
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
                            return Ok(Some(Self::connect(socket_path).await?));
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
                return Ok(Some(Self::connect(path).await?));
            }
        }

        debug!("No nvim socket discovered");
        Ok(None)
    }

    /// Get the socket path this instance is connected to
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
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
        let lua = r#"
            local target_path = ...
            for _, buf in ipairs(vim.api.nvim_list_bufs()) do
                local name = vim.api.nvim_buf_get_name(buf)
                if name == target_path then
                    vim.api.nvim_buf_call(buf, function()
                        vim.cmd('checktime')
                    end)
                end
            end
        "#;
        self.exec_lua(lua, args).await?;
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

        // Pass original lines, modified lines, title, and language as args
        let original_lines: Vec<Value> = original.lines().map(|l| Value::from(l)).collect();
        let modified_lines: Vec<Value> = modified.lines().map(|l| Value::from(l)).collect();
        let args = vec![
            Value::from(original_lines),
            Value::from(modified_lines),
            Value::from(title),
            Value::from(language.unwrap_or("")),
        ];

        let lua = r#"
            local original_lines, modified_lines, title, lang = ...
            
            -- Close any existing preview first
            if vim.g.codey_preview_tab and vim.api.nvim_tabpage_is_valid(vim.g.codey_preview_tab) then
                local tab_nr = vim.api.nvim_tabpage_get_number(vim.g.codey_preview_tab)
                if #vim.api.nvim_list_tabpages() > 1 then
                    vim.cmd('tabclose ' .. tab_nr)
                end
            end
            
            -- Remember current tab to return to later
            local original_tab = vim.api.nvim_get_current_tabpage()
            vim.g.codey_original_tab = original_tab
            
            -- Create a new tab for the preview
            vim.cmd('tabnew')
            local preview_tab = vim.api.nvim_get_current_tabpage()
            vim.g.codey_preview_tab = preview_tab
            
            -- Left buffer: original content
            local left_buf = vim.api.nvim_get_current_buf()
            vim.bo[left_buf].buftype = 'nofile'
            vim.bo[left_buf].bufhidden = 'wipe'
            vim.bo[left_buf].swapfile = false
            vim.api.nvim_buf_set_name(left_buf, '[Codey] ' .. title .. ' (original)')
            vim.api.nvim_buf_set_lines(left_buf, 0, -1, false, original_lines)
            vim.bo[left_buf].modifiable = false
            vim.bo[left_buf].readonly = true
            if lang ~= '' then
                vim.bo[left_buf].filetype = lang
            end
            
            -- Enable diff mode on left buffer
            vim.cmd('diffthis')
            
            -- Create vertical split for right buffer (modified content)
            -- rightbelow ensures new split is on the right
            vim.cmd('rightbelow vsplit')
            local right_buf = vim.api.nvim_create_buf(false, true)
            vim.api.nvim_win_set_buf(0, right_buf)
            vim.bo[right_buf].buftype = 'nofile'
            vim.bo[right_buf].bufhidden = 'wipe'
            vim.bo[right_buf].swapfile = false
            vim.api.nvim_buf_set_name(right_buf, '[Codey] ' .. title .. ' (modified)')
            vim.api.nvim_buf_set_lines(right_buf, 0, -1, false, modified_lines)
            vim.bo[right_buf].modifiable = false
            vim.bo[right_buf].readonly = true
            if lang ~= '' then
                vim.bo[right_buf].filetype = lang
            end
            
            -- Enable diff mode on right buffer
            vim.cmd('diffthis')
            
            -- Store buffer handles for cleanup
            vim.g.codey_preview_buf = left_buf
            vim.g.codey_preview_buf_right = right_buf
            
            -- Helper function to close preview and return to original tab
            local function close_preview()
                vim.g.codey_preview_tab = nil
                vim.g.codey_preview_buf = nil
                vim.g.codey_preview_buf_right = nil
                vim.cmd('tabclose')
                local orig = vim.g.codey_original_tab
                vim.g.codey_original_tab = nil
                if orig and vim.api.nvim_tabpage_is_valid(orig) then
                    vim.api.nvim_set_current_tabpage(orig)
                end
            end
            
            -- Map 'q' to close on both buffers
            vim.keymap.set('n', 'q', close_preview, { buffer = left_buf, silent = true })
            vim.keymap.set('n', 'q', close_preview, { buffer = right_buf, silent = true })
        "#;

        self.exec_lua(lua, args).await?;
        info!("Displayed side-by-side diff in nvim: {}", title);
        Ok(())
    }

    /// Close any open Codey preview buffers - internal helper
    async fn close_diff_buffers(&self) -> Result<()> {
        let lua = r#"
            -- Close preview tab if we have one tracked
            local preview_tab = vim.g.codey_preview_tab
            local original_tab = vim.g.codey_original_tab
            
            if preview_tab and vim.api.nvim_tabpage_is_valid(preview_tab) then
                local tab_nr = vim.api.nvim_tabpage_get_number(preview_tab)
                -- Don't close if it's the last tab
                if #vim.api.nvim_list_tabpages() > 1 then
                    vim.cmd('tabclose ' .. tab_nr)
                else
                    -- Last tab, just delete the buffers
                    if vim.g.codey_preview_buf and vim.api.nvim_buf_is_valid(vim.g.codey_preview_buf) then
                        vim.api.nvim_buf_delete(vim.g.codey_preview_buf, { force = true })
                    end
                    if vim.g.codey_preview_buf_right and vim.api.nvim_buf_is_valid(vim.g.codey_preview_buf_right) then
                        vim.api.nvim_buf_delete(vim.g.codey_preview_buf_right, { force = true })
                    end
                end
            end
            
            -- Return to original tab if valid
            if original_tab and vim.api.nvim_tabpage_is_valid(original_tab) then
                vim.api.nvim_set_current_tabpage(original_tab)
            end
            
            -- Clear state
            vim.g.codey_preview_tab = nil
            vim.g.codey_preview_buf = nil
            vim.g.codey_preview_buf_right = nil
            vim.g.codey_original_tab = nil
        "#;
        self.exec_lua(lua, vec![]).await?;
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

        let lua = r#"
            local lines, title, lang = ...
            
            -- Close any existing preview first
            if vim.g.codey_preview_tab and vim.api.nvim_tabpage_is_valid(vim.g.codey_preview_tab) then
                local tab_nr = vim.api.nvim_tabpage_get_number(vim.g.codey_preview_tab)
                if #vim.api.nvim_list_tabpages() > 1 then
                    vim.cmd('tabclose ' .. tab_nr)
                end
            end
            
            -- Remember current tab to return to later
            local original_tab = vim.api.nvim_get_current_tabpage()
            vim.g.codey_original_tab = original_tab
            
            -- Create a new tab for the preview
            vim.cmd('tabnew')
            local preview_tab = vim.api.nvim_get_current_tabpage()
            vim.g.codey_preview_tab = preview_tab
            
            -- Set buffer options
            local buf = vim.api.nvim_get_current_buf()
            vim.g.codey_preview_buf = buf
            vim.bo[buf].buftype = 'nofile'
            vim.bo[buf].bufhidden = 'wipe'
            vim.bo[buf].swapfile = false
            
            -- Set filetype for syntax highlighting
            if lang ~= '' then
                vim.bo[buf].filetype = lang
            end
            
            -- Set buffer name
            vim.api.nvim_buf_set_name(buf, '[Codey] ' .. title)
            
            -- Set the lines directly
            vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
            
            -- Make buffer readonly
            vim.bo[buf].modifiable = false
            vim.bo[buf].readonly = true
            
            -- Map 'q' to close the tab and return to original
            vim.keymap.set('n', 'q', function()
                vim.g.codey_preview_tab = nil
                vim.g.codey_preview_buf = nil
                vim.cmd('tabclose')
                local orig = vim.g.codey_original_tab
                vim.g.codey_original_tab = nil
                if orig and vim.api.nvim_tabpage_is_valid(orig) then
                    vim.api.nvim_set_current_tabpage(orig)
                end
            end, { buffer = buf, silent = true })
        "#;

        self.exec_lua(lua, args).await?;
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

        let lua = r#"
            local path, line, col = ...
            vim.cmd('edit ' .. vim.fn.fnameescape(path))
            vim.api.nvim_win_set_cursor(0, {line, col - 1})
        "#;

        self.exec_lua(lua, args).await?;
        Ok(())
    }

    async fn notify(&self, message: &str, level: NotifyLevel) -> Result<()> {
        let level_num = match level {
            NotifyLevel::Info => 2,
            NotifyLevel::Warn => 3,
            NotifyLevel::Error => 4,
        };

        let args = vec![Value::from(message), Value::from(level_num)];
        let lua = r#"
            local msg, level = ...
            vim.notify(msg, level, { title = "Codey" })
        "#;

        self.exec_lua(lua, args).await?;
        Ok(())
    }

    async fn get_selection(&self) -> Result<Option<Selection>> {
        let lua = r#"
            -- Check if we're in visual mode or have a recent selection
            local mode = vim.fn.mode()
            local start_pos = vim.fn.getpos("'<")
            local end_pos = vim.fn.getpos("'>")
            
            -- If marks are not set (line 0), no selection
            if start_pos[2] == 0 or end_pos[2] == 0 then
                return nil
            end
            
            local lines = vim.fn.getline(start_pos[2], end_pos[2])
            if type(lines) == 'string' then
                lines = {lines}
            end
            
            -- Handle partial line selections
            if #lines > 0 then
                -- Adjust last line for end column
                if end_pos[3] < #lines[#lines] then
                    lines[#lines] = lines[#lines]:sub(1, end_pos[3])
                end
                -- Adjust first line for start column
                if start_pos[3] > 1 then
                    lines[1] = lines[1]:sub(start_pos[3])
                end
            end
            
            return {
                path = vim.fn.expand('%:p'),
                content = table.concat(lines, '\n'),
                start_line = start_pos[2],
                end_line = end_pos[2],
                start_col = start_pos[3],
                end_col = end_pos[3],
            }
        "#;

        let result = self.exec_lua(lua, vec![]).await?;

        if result.is_nil() {
            return Ok(None);
        }

        // Parse the result table
        let map = result.as_map().context("Expected map from get_selection")?;
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

        Ok(Some(Selection {
            path: get_str("path").unwrap_or_default(),
            content: get_str("content").unwrap_or_default(),
            start_line: get_u32("start_line").unwrap_or(0),
            end_line: get_u32("end_line").unwrap_or(0),
            start_col: get_u32("start_col"),
            end_col: get_u32("end_col"),
        }))
    }

    async fn get_current_file(&self) -> Result<Option<CurrentFile>> {
        let lua = r#"
            local path = vim.fn.expand('%:p')
            if path == '' then
                return nil
            end
            
            local cursor = vim.api.nvim_win_get_cursor(0)
            return {
                path = path,
                cursor_line = cursor[1],
                cursor_col = cursor[2] + 1,
                total_lines = vim.api.nvim_buf_line_count(0),
            }
        "#;

        let result = self.exec_lua(lua, vec![]).await?;

        if result.is_nil() {
            return Ok(None);
        }

        let map = result.as_map().context("Expected map from get_current_file")?;
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

        Ok(Some(CurrentFile {
            path: get_str("path").unwrap_or_default(),
            cursor_line: get_u32("cursor_line").unwrap_or(1),
            cursor_col: get_u32("cursor_col").unwrap_or(1),
            total_lines: get_u32("total_lines").unwrap_or(0),
        }))
    }

    async fn get_visible_range(&self) -> Result<Option<VisibleRange>> {
        let lua = r#"
            local path = vim.fn.expand('%:p')
            if path == '' then
                return nil
            end
            
            local win = vim.api.nvim_get_current_win()
            local top = vim.fn.line('w0')
            local bottom = vim.fn.line('w$')
            
            return {
                path = path,
                start_line = top,
                end_line = bottom,
            }
        "#;

        let result = self.exec_lua(lua, vec![]).await?;

        if result.is_nil() {
            return Ok(None);
        }

        let map = result.as_map().context("Expected map from get_visible_range")?;
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

        Ok(Some(VisibleRange {
            path: get_str("path").unwrap_or_default(),
            start_line: get_u32("start_line").unwrap_or(1),
            end_line: get_u32("end_line").unwrap_or(1),
        }))
    }

    async fn has_unsaved_changes(&self, path: &str) -> Result<bool> {
        let args = vec![Value::from(path)];
        let lua = r#"
            local target_path = ...
            for _, buf in ipairs(vim.api.nvim_list_bufs()) do
                local name = vim.api.nvim_buf_get_name(buf)
                if name == target_path and vim.bo[buf].modified then
                    return true
                end
            end
            return false
        "#;

        let result = self.exec_lua(lua, args).await?;
        Ok(result.as_bool().unwrap_or(false))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires running nvim instance
    async fn test_discover_nvim() {
        let nvim = Nvim::discover(None).await.unwrap();
        if let Some(nvim) = nvim {
            println!("Connected to {} at {:?}", nvim.name(), nvim.socket_path());
        } else {
            println!("No nvim instance found");
        }
    }

    #[tokio::test]
    #[ignore] // Requires running nvim instance
    async fn test_show_preview() {
        let nvim = Nvim::discover(None).await.unwrap().expect("Need nvim");
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
        let nvim = Nvim::discover(None).await.unwrap().expect("Need nvim");
        let file = nvim.get_current_file().await.unwrap();
        println!("Current file: {:?}", file);
    }

    #[tokio::test]
    #[ignore] // Requires running nvim instance
    async fn test_get_selection() {
        let nvim = Nvim::discover(None).await.unwrap().expect("Need nvim");
        let selection = nvim.get_selection().await.unwrap();
        println!("Selection: {:?}", selection);
    }
}
