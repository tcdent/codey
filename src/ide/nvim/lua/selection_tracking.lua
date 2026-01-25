-- Selection tracking for Codey
-- Sets up autocommands to notify the app when visual selection changes
-- Broadcasts to all connected codey instances via their channel IDs
-- Requires: vim.g.codey_channel_id to be set before loading (for registering this instance)

local group = vim.api.nvim_create_augroup('CodeySelection', { clear = true })

-- Register this channel in the global list (supports multiple codey instances)
vim.g.codey_channels = vim.g.codey_channels or {}
local channel_id = vim.g.codey_channel_id
local already_registered = false
for _, ch in ipairs(vim.g.codey_channels) do
    if ch == channel_id then
        already_registered = true
        break
    end
end
if not already_registered then
    local channels = vim.g.codey_channels
    table.insert(channels, channel_id)
    vim.g.codey_channels = channels
end

-- Broadcast to all connected codey instances
local function broadcast(event_name, data)
    for _, ch_id in ipairs(vim.g.codey_channels or {}) do
        -- pcall to ignore dead channels (instance disconnected)
        pcall(vim.rpcnotify, ch_id, event_name, data)
    end
end

local function send_selection()
    local mode = vim.fn.mode()
    -- Only send in visual modes (v, V, CTRL-V)
    if mode ~= 'v' and mode ~= 'V' and mode ~= '\22' then
        broadcast('codey_selection', nil)
        return
    end

    -- Get visual selection range
    local start_pos = vim.fn.getpos('v')
    local end_pos = vim.fn.getpos('.')

    -- Normalize: ensure start is before end
    if start_pos[2] > end_pos[2] or
       (start_pos[2] == end_pos[2] and start_pos[3] > end_pos[3]) then
        start_pos, end_pos = end_pos, start_pos
    end

    local start_line = start_pos[2]
    local end_line = end_pos[2]
    local start_col = start_pos[3]
    local end_col = end_pos[3]

    -- Get the content (for line-wise selection, get full lines)
    local lines
    if mode == 'V' then
        lines = vim.api.nvim_buf_get_lines(0, start_line - 1, end_line, false)
    else
        lines = vim.fn.getline(start_line, end_line)
        if type(lines) == 'string' then
            lines = {lines}
        end
    end

    local path = vim.fn.expand('%:p')

    broadcast('codey_selection', {
        path = path,
        content = table.concat(lines, '\n'),
        start_line = start_line,
        end_line = end_line,
        start_col = start_col,
        end_col = end_col,
    })
end

local function clear_selection()
    broadcast('codey_selection', nil)
end

vim.api.nvim_create_autocmd('ModeChanged', {
    group = group,
    pattern = '*:[vV\x16]*',  -- Entering visual mode
    callback = function()
        vim.defer_fn(send_selection, 10)
    end,
})

vim.api.nvim_create_autocmd('CursorMoved', {
    group = group,
    callback = function()
        local mode = vim.fn.mode()
        if mode == 'v' or mode == 'V' or mode == '\22' then
            send_selection()
        end
    end,
})

vim.api.nvim_create_autocmd('ModeChanged', {
    group = group,
    pattern = '[vV\x16]:*',  -- Leaving visual mode
    callback = clear_selection,
})
