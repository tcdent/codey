-- Close any open Codey preview buffers/tabs

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
