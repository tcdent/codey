-- Check if a file has unsaved changes
-- Args: target_path (string)

local target_path = ...

-- Normalize the target path to absolute
local normalized_target = vim.fn.fnamemodify(target_path, ':p')

for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_is_loaded(buf) then
        local buf_name = vim.api.nvim_buf_get_name(buf)
        -- Normalize buffer name the same way
        local normalized_buf = vim.fn.fnamemodify(buf_name, ':p')
        if normalized_buf == normalized_target and vim.bo[buf].modified then
            return true
        end
    end
end
return false
