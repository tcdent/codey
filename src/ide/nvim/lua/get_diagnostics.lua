-- Get diagnostics for a file, waiting for LSP to process after reload
-- Args: target_path (string), timeout_ms (number)
-- Returns: array of diagnostics

local target_path, timeout_ms = ...
timeout_ms = timeout_ms or 500

-- Find the buffer
local bufnr = nil
for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_get_name(buf) == target_path then
        bufnr = buf
        break
    end
end

if not bufnr then
    return {}
end

-- Track if we've received new diagnostics
local received = false
local autocmd_id = vim.api.nvim_create_autocmd('DiagnosticChanged', {
    buffer = bufnr,
    once = true,
    callback = function()
        received = true
    end,
})

-- Reload the buffer to trigger LSP analysis
vim.api.nvim_buf_call(bufnr, function()
    vim.cmd('checktime')
end)

-- Wait for DiagnosticChanged or timeout
-- The 10ms interval allows Neovim to process events
vim.wait(timeout_ms, function()
    return received
end, 10)

-- Clean up autocmd if it didn't fire
pcall(vim.api.nvim_del_autocmd, autocmd_id)

-- Get current diagnostics for the buffer
local diagnostics = vim.diagnostic.get(bufnr)
local result = {}

for _, d in ipairs(diagnostics) do
    table.insert(result, {
        line = d.lnum + 1,  -- Convert 0-indexed to 1-indexed
        col = d.col + 1,
        end_line = d.end_lnum and (d.end_lnum + 1) or nil,
        end_col = d.end_col and (d.end_col + 1) or nil,
        severity = d.severity,  -- 1=Error, 2=Warning, 3=Info, 4=Hint
        message = d.message,
        source = d.source,
    })
end

return result
