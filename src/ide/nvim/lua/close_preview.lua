-- Close any open Codey preview buffers/tabs
-- Safe to call even if no preview is open (no-op)

local preview_tab = vim.g.codey_preview_tab
local original_tab = vim.g.codey_original_tab

-- Early return if no preview was opened
if not preview_tab then
    return
end

-- Check if preview tab is still valid
if not vim.api.nvim_tabpage_is_valid(preview_tab) then
    -- Already closed, just clear state
    vim.g.codey_preview_tab = nil
    vim.g.codey_preview_buf = nil
    vim.g.codey_preview_buf_right = nil
    vim.g.codey_original_tab = nil
    vim.g.codey_preview_owner = nil
    return
end

-- Switch to preview tab to clean up properly
vim.api.nvim_set_current_tabpage(preview_tab)

-- Turn off diff mode in all windows of this tab to prevent crash
for _, win in ipairs(vim.api.nvim_tabpage_list_wins(preview_tab)) do
    if vim.api.nvim_win_is_valid(win) then
        vim.api.nvim_win_call(win, function()
            vim.cmd('diffoff')
        end)
    end
end

-- Delete preview buffers (force, no save prompt)
local bufs_to_delete = {}
if vim.g.codey_preview_buf and vim.api.nvim_buf_is_valid(vim.g.codey_preview_buf) then
    table.insert(bufs_to_delete, vim.g.codey_preview_buf)
end
if vim.g.codey_preview_buf_right and vim.api.nvim_buf_is_valid(vim.g.codey_preview_buf_right) then
    table.insert(bufs_to_delete, vim.g.codey_preview_buf_right)
end

-- Return to original tab first (if valid)
if original_tab and vim.api.nvim_tabpage_is_valid(original_tab) then
    vim.api.nvim_set_current_tabpage(original_tab)
end

-- Now close the preview tab (if not the last tab)
if #vim.api.nvim_list_tabpages() > 1 then
    local tab_nr = vim.api.nvim_tabpage_get_number(preview_tab)
    vim.cmd('tabclose ' .. tab_nr)
end

-- Delete the buffers after tab is closed
for _, buf in ipairs(bufs_to_delete) do
    if vim.api.nvim_buf_is_valid(buf) then
        vim.api.nvim_buf_delete(buf, { force = true })
    end
end

-- Clear state
vim.g.codey_preview_tab = nil
vim.g.codey_preview_buf = nil
vim.g.codey_preview_buf_right = nil
vim.g.codey_original_tab = nil
vim.g.codey_preview_owner = nil
