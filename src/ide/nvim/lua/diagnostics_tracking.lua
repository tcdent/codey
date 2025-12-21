-- Diagnostics tracking for Codey
-- Sets up autocommands to notify the app when LSP diagnostics change
-- Requires: vim.g.codey_channel_id to be set before loading

local group = vim.api.nvim_create_augroup('CodeyDiagnostics', { clear = true })
local channel_id = vim.g.codey_channel_id

-- Debounce timer to avoid flooding with rapid diagnostic updates
local debounce_timer = nil
local debounce_ms = 100

local function send_diagnostics(bufnr)
    bufnr = bufnr or vim.api.nvim_get_current_buf()

    -- Only send for real files
    local path = vim.api.nvim_buf_get_name(bufnr)
    if path == '' or vim.bo[bufnr].buftype ~= '' then
        return
    end

    local diagnostics = vim.diagnostic.get(bufnr)
    if #diagnostics == 0 then
        -- Send empty list to clear diagnostics for this file
        vim.rpcnotify(channel_id, 'codey_diagnostics', {
            path = path,
            diagnostics = {},
        })
        return
    end

    local formatted = {}
    for _, d in ipairs(diagnostics) do
        table.insert(formatted, {
            line = d.lnum + 1,  -- Convert 0-indexed to 1-indexed
            col = d.col + 1,
            end_line = d.end_lnum and (d.end_lnum + 1) or nil,
            end_col = d.end_col and (d.end_col + 1) or nil,
            severity = d.severity,  -- 1=Error, 2=Warning, 3=Info, 4=Hint
            message = d.message,
            source = d.source,
        })
    end

    vim.rpcnotify(channel_id, 'codey_diagnostics', {
        path = path,
        diagnostics = formatted,
    })
end

local function send_diagnostics_debounced(bufnr)
    if debounce_timer then
        vim.fn.timer_stop(debounce_timer)
    end
    debounce_timer = vim.fn.timer_start(debounce_ms, function()
        debounce_timer = nil
        send_diagnostics(bufnr)
    end)
end

-- Send diagnostics when they change (LSP updates, linter runs, etc.)
vim.api.nvim_create_autocmd('DiagnosticChanged', {
    group = group,
    callback = function(args)
        send_diagnostics_debounced(args.buf)
    end,
})
