-- Display a side-by-side diff using nvim's built-in diff mode
-- Args: original_lines (table), modified_lines (table), title (string), lang (string)

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
