-- Display file content in a scratch buffer
-- Args: lines (table), title (string), lang (string), channel_id (number)

local lines, title, lang, channel_id = ...

-- Close any existing preview first
if vim.g.codey_preview_tab and vim.api.nvim_tabpage_is_valid(vim.g.codey_preview_tab) then
    local tab_nr = vim.api.nvim_tabpage_get_number(vim.g.codey_preview_tab)
    if #vim.api.nvim_list_tabpages() > 1 then
        vim.cmd('tabclose ' .. tab_nr)
    end
end

-- Set owner to this channel
vim.g.codey_preview_owner = channel_id

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
    vim.g.codey_preview_owner = nil
    vim.cmd('tabclose')
    local orig = vim.g.codey_original_tab
    vim.g.codey_original_tab = nil
    if orig and vim.api.nvim_tabpage_is_valid(orig) then
        vim.api.nvim_set_current_tabpage(orig)
    end
end, { buffer = buf, silent = true })
